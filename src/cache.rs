use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::config::AudioCodec;

const STATE_FILE: &str = ".cache-state.json";

#[derive(Clone)]
pub struct AudioCache {
    directory: PathBuf,
    lifetime: Duration,
    signature: CacheSignature,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CacheSignature {
    pub engine_url: String,
    pub cache_revision: u64,
    pub codec: AudioCodec,
    pub bitrate_kbps: u32,
}

impl AudioCache {
    pub fn new(directory: PathBuf, cache_days: u64, signature: CacheSignature) -> Self {
        Self {
            directory,
            lifetime: Duration::from_secs(cache_days * 86_400),
            signature,
        }
    }

    pub async fn prepare(&self) -> Result<()> {
        self.ensure_ready().await?;
        self.cleanup_files().await
    }

    pub async fn ensure_ready(&self) -> Result<()> {
        fs::create_dir_all(&self.directory).await.with_context(|| {
            format!(
                "キャッシュディレクトリ {:?} を作成できません",
                self.directory
            )
        })?;

        let state_path = self.directory.join(STATE_FILE);
        let previous = match fs::read(&state_path).await {
            Ok(bytes) => serde_json::from_slice::<CacheSignature>(&bytes).ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("キャッシュ状態を読み込めません"),
        };

        if previous.as_ref() != Some(&self.signature) {
            self.clear_audio_files().await?;
            self.write_state().await?;
        }

        Ok(())
    }

    pub fn audio_path(&self, audio_id: &str) -> PathBuf {
        self.directory.join(format!("{audio_id}.ogg"))
    }

    pub fn temporary_path(&self, audio_id: &str) -> PathBuf {
        self.directory.join(format!("{audio_id}.ogg.tmp"))
    }

    pub async fn find(&self, audio_id: &str) -> Result<Option<PathBuf>> {
        let path = self.audio_path(audio_id);
        match fs::metadata(&path).await {
            Ok(metadata) if self.is_fresh(&metadata) => Ok(Some(path)),
            Ok(_) => {
                remove_if_exists(&path).await?;
                Ok(None)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("キャッシュファイルを確認できません"),
        }
    }

    pub async fn cleanup(&self) -> Result<()> {
        self.ensure_ready().await?;
        self.cleanup_files().await
    }

    async fn cleanup_files(&self) -> Result<()> {
        let mut entries = fs::read_dir(&self.directory)
            .await
            .context("キャッシュディレクトリを確認できません")?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if is_temporary_file(&path) {
                remove_if_exists(&path).await?;
                continue;
            }
            if !is_audio_file(&path) {
                continue;
            }
            let metadata = entry.metadata().await?;
            if !self.is_fresh(&metadata) {
                remove_if_exists(&path).await?;
            }
        }

        Ok(())
    }

    async fn clear_audio_files(&self) -> Result<()> {
        let mut entries = fs::read_dir(&self.directory)
            .await
            .context("キャッシュディレクトリを確認できません")?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if is_audio_file(&path) || is_temporary_file(&path) {
                remove_if_exists(&path).await?;
            }
        }
        Ok(())
    }

    async fn write_state(&self) -> Result<()> {
        let state_path = self.directory.join(STATE_FILE);
        let temporary_path = self.directory.join(format!("{STATE_FILE}.tmp"));
        let bytes = serde_json::to_vec_pretty(&self.signature)?;

        fs::write(&temporary_path, bytes)
            .await
            .context("一時キャッシュ状態を書き込めません")?;
        remove_if_exists(&state_path).await?;
        fs::rename(&temporary_path, &state_path)
            .await
            .context("キャッシュ状態を確定できません")
    }

    fn is_fresh(&self, metadata: &std::fs::Metadata) -> bool {
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        age < self.lifetime
    }
}

fn is_audio_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("ogg")
}

fn is_temporary_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("tmp")
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("ファイル {path:?} を削除できません")),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use tokio::fs;

    use super::{AudioCache, CacheSignature, STATE_FILE};
    use crate::config::AudioCodec;

    #[tokio::test]
    async fn 削除されたキャッシュディレクトリを再作成する() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tts-server-cache-test-{}-{unique}",
            std::process::id()
        ));
        let cache = AudioCache::new(
            directory.clone(),
            7,
            CacheSignature {
                engine_url: "http://127.0.0.1:10101".to_owned(),
                cache_revision: 1,
                codec: AudioCodec::Opus,
                bitrate_kbps: 48,
            },
        );

        cache.prepare().await.unwrap();
        fs::remove_dir_all(&directory).await.unwrap();
        cache.ensure_ready().await.unwrap();

        assert!(directory.is_dir());
        assert!(directory.join(STATE_FILE).is_file());
        fs::remove_dir_all(directory).await.unwrap();
    }
}
