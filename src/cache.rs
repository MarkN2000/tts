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
    max_size_bytes: u64,
    signature: CacheSignature,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct CacheUsage {
    pub file_count: u64,
    pub used_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CacheSignature {
    pub engine_url: String,
    pub cache_revision: u64,
    pub codec: AudioCodec,
    pub bitrate_kbps: u32,
}

impl AudioCache {
    pub fn new(
        directory: PathBuf,
        cache_days: u64,
        cache_max_mb: u64,
        signature: CacheSignature,
    ) -> Self {
        Self {
            directory,
            lifetime: Duration::from_secs(cache_days * 86_400),
            max_size_bytes: cache_max_mb * 1024 * 1024,
            signature,
        }
    }

    pub async fn prepare(&self) -> Result<()> {
        self.ensure_ready().await?;
        self.cleanup_files(None).await
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
        self.cleanup_files(None).await
    }

    pub async fn cleanup_after_generation(&self, audio_id: &str) -> Result<()> {
        self.cleanup_files(Some(audio_id)).await
    }

    pub async fn clear(&self) -> Result<()> {
        self.ensure_ready().await?;
        self.clear_audio_files().await
    }

    pub async fn usage(&self) -> Result<CacheUsage> {
        let mut entries = match fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheUsage::default());
            }
            Err(error) => return Err(error).context("キャッシュディレクトリを確認できません"),
        };
        let mut usage = CacheUsage::default();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !is_audio_file(&path) {
                continue;
            }
            let metadata = match entry.metadata().await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("キャッシュファイルを確認できません"),
            };
            if !metadata.is_file() {
                continue;
            }
            usage.file_count = usage.file_count.saturating_add(1);
            usage.used_bytes = usage.used_bytes.saturating_add(metadata.len());
        }

        Ok(usage)
    }

    async fn cleanup_files(&self, preserved_audio_id: Option<&str>) -> Result<()> {
        let mut entries = fs::read_dir(&self.directory)
            .await
            .context("キャッシュディレクトリを確認できません")?;
        let mut total_size = 0_u64;
        let mut eviction_candidates = Vec::new();

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
                continue;
            }
            total_size = total_size.saturating_add(metadata.len());
            let is_preserved =
                path.file_stem().and_then(|value| value.to_str()) == preserved_audio_id;
            if !is_preserved {
                eviction_candidates.push((
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                    path,
                ));
            }
        }

        eviction_candidates.sort_by_key(|(modified, _, _)| *modified);
        for (_, size, path) in eviction_candidates {
            if total_size <= self.max_size_bytes {
                break;
            }
            remove_if_exists(&path).await?;
            total_size = total_size.saturating_sub(size);
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
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use tokio::{fs, time::sleep};

    use super::{AudioCache, CacheSignature, STATE_FILE};
    use crate::config::AudioCodec;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn 削除されたキャッシュディレクトリを再作成する() {
        let (cache, directory) = test_cache(1024);

        cache.prepare().await.unwrap();
        fs::remove_dir_all(&directory).await.unwrap();
        cache.ensure_ready().await.unwrap();

        assert!(directory.is_dir());
        assert!(directory.join(STATE_FILE).is_file());
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 容量超過時は古い音声から削除する() {
        let (cache, directory) = test_cache(1);
        cache.prepare().await.unwrap();

        fs::write(cache.audio_path("old"), vec![0; 450 * 1024])
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
        fs::write(cache.audio_path("middle"), vec![0; 450 * 1024])
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
        fs::write(cache.audio_path("new"), vec![0; 450 * 1024])
            .await
            .unwrap();

        cache.cleanup_after_generation("new").await.unwrap();

        assert!(!cache.audio_path("old").is_file());
        assert!(cache.audio_path("middle").is_file());
        assert!(cache.audio_path("new").is_file());
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 使用量にはoggだけを含める() {
        let (cache, directory) = test_cache(1);
        cache.prepare().await.unwrap();
        fs::write(cache.audio_path("one"), vec![0; 100])
            .await
            .unwrap();
        fs::write(cache.audio_path("two"), vec![0; 250])
            .await
            .unwrap();
        fs::write(cache.temporary_path("working"), vec![0; 500])
            .await
            .unwrap();

        let usage = cache.usage().await.unwrap();

        assert_eq!(usage.file_count, 2);
        assert_eq!(usage.used_bytes, 350);
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 存在しないキャッシュディレクトリの使用量は0になる() {
        let (cache, directory) = test_cache(1);

        assert_eq!(cache.usage().await.unwrap(), Default::default());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn キャッシュ削除後も状態ファイルを維持する() {
        let (cache, directory) = test_cache(1);
        cache.prepare().await.unwrap();
        let audio = cache.audio_path("audio");
        let temporary = cache.temporary_path("working");
        fs::write(&audio, vec![0; 100]).await.unwrap();
        fs::write(&temporary, vec![0; 100]).await.unwrap();

        cache.clear().await.unwrap();

        assert!(!audio.exists());
        assert!(!temporary.exists());
        assert!(directory.join(STATE_FILE).is_file());
        fs::remove_dir_all(directory).await.unwrap();
    }

    fn test_cache(cache_max_mb: u64) -> (AudioCache, PathBuf) {
        let unique = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "tts-server-cache-test-{}-{unique}",
            std::process::id()
        ));
        let cache = AudioCache::new(
            directory.clone(),
            7,
            cache_max_mb,
            CacheSignature {
                engine_url: "http://127.0.0.1:10101".to_owned(),
                cache_revision: 1,
                codec: AudioCodec::Opus,
                bitrate_kbps: 48,
            },
        );
        (cache, directory)
    }
}
