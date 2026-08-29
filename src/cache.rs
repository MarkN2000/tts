use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncWriteExt};

use crate::{
    atomic_file::{replace_file, sync_directory},
    config::AudioCodec,
};

const STATE_FILE: &str = ".cache-state.json";
const ROOT_STATE_FILE: &str = ".cache-engines.json";

#[derive(Clone)]
pub struct AudioCache {
    directory: PathBuf,
    lifetime: Duration,
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

#[derive(Debug, Default, Deserialize, Serialize)]
struct CacheRootState {
    engine_ids: Vec<String>,
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
        cleanup_all(&[self], u64::MAX, None).await
    }

    pub async fn ensure_ready(&self) -> Result<()> {
        match fs::symlink_metadata(&self.directory).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "キャッシュディレクトリ {:?} がシンボリックリンクです",
                    self.directory
                );
            }
            Ok(metadata) if !metadata.is_dir() => {
                anyhow::bail!(
                    "キャッシュディレクトリ {:?} がディレクトリではありません",
                    self.directory
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.directory).await.with_context(|| {
                    format!(
                        "キャッシュディレクトリ {:?} を作成できません",
                        self.directory
                    )
                })?;
            }
            Err(error) => return Err(error).context("キャッシュディレクトリを確認できません"),
        }

        let state_path = self.directory.join(STATE_FILE);
        let previous = match fs::read(&state_path).await {
            Ok(bytes) => Some(
                serde_json::from_slice::<CacheSignature>(&bytes)
                    .context("キャッシュ状態が不正です")?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut entries = fs::read_dir(&self.directory)
                    .await
                    .context("キャッシュディレクトリを確認できません")?;
                if entries.next_entry().await?.is_some() {
                    anyhow::bail!(
                        "管理状態のない既存ディレクトリをEngineキャッシュとして使用できません: {:?}",
                        self.directory
                    );
                }
                None
            }
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

    pub async fn clear(&self) -> Result<()> {
        self.ensure_ready().await?;
        self.clear_audio_files().await
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
        write_atomic_state(&state_path, &temporary_path, &bytes, "キャッシュ状態").await
    }

    fn is_fresh(&self, metadata: &std::fs::Metadata) -> bool {
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        age < self.lifetime
    }
}

pub async fn prepare_cache_root(root: &Path, active_engine_ids: &[String]) -> Result<()> {
    fs::create_dir_all(root)
        .await
        .with_context(|| format!("キャッシュディレクトリ {root:?} を作成できません"))?;
    let active = active_engine_ids
        .iter()
        .map(OsString::from)
        .collect::<HashSet<_>>();
    let root_state_path = root.join(ROOT_STATE_FILE);
    let previous = match fs::read(&root_state_path).await {
        Ok(bytes) => serde_json::from_slice::<CacheRootState>(&bytes)
            .context("キャッシュルート状態が不正です")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CacheRootState::default(),
        Err(error) => return Err(error).context("キャッシュルート状態を読み込めません"),
    };
    for engine_id in previous.engine_ids {
        if active.contains(&OsString::from(&engine_id)) {
            continue;
        }
        if !is_valid_engine_id(&engine_id) {
            anyhow::bail!("キャッシュルート状態のEngine IDが不正です");
        }
        let path = root.join(&engine_id);
        match fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("Engineキャッシュ {path:?} がシンボリックリンクです");
            }
            Ok(metadata) if metadata.is_dir() => {
                let state_path = path.join(STATE_FILE);
                let owned = fs::read(&state_path)
                    .await
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<CacheSignature>(&bytes).ok())
                    .is_some();
                if owned {
                    fs::remove_dir_all(&path)
                        .await
                        .with_context(|| format!("旧Engineキャッシュ {path:?} を削除できません"))?;
                }
            }
            Ok(_) => anyhow::bail!("Engineキャッシュ {path:?} がディレクトリではありません"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("旧Engineキャッシュ {path:?} を確認できません"));
            }
        }
    }

    let mut entries = fs::read_dir(root)
        .await
        .context("キャッシュディレクトリを確認できません")?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let file_type = entry.file_type().await?;
        if file_type.is_file()
            && (is_legacy_audio_file(&path)
                || is_legacy_temporary_file(&path)
                || path.file_name().and_then(|value| value.to_str()) == Some(STATE_FILE))
        {
            remove_if_exists(&path).await?;
        }
    }

    let state = CacheRootState {
        engine_ids: active_engine_ids.to_vec(),
    };
    let temporary_path = root.join(format!("{ROOT_STATE_FILE}.tmp"));
    write_atomic_state(
        &root_state_path,
        &temporary_path,
        &serde_json::to_vec_pretty(&state)?,
        "キャッシュルート状態",
    )
    .await?;
    Ok(())
}

pub async fn cleanup_all(
    caches: &[&AudioCache],
    max_size_bytes: u64,
    preserved_path: Option<&Path>,
) -> Result<()> {
    let mut total_size = 0_u64;
    let mut eviction_candidates = Vec::new();

    for cache in caches {
        cache.ensure_ready().await?;
        let mut entries = fs::read_dir(&cache.directory)
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
            if !cache.is_fresh(&metadata) {
                remove_if_exists(&path).await?;
                continue;
            }
            total_size = total_size.saturating_add(metadata.len());
            if preserved_path != Some(path.as_path()) {
                eviction_candidates.push((
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                    path,
                ));
            }
        }
    }

    eviction_candidates.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in eviction_candidates {
        if total_size <= max_size_bytes {
            break;
        }
        remove_if_exists(&path).await?;
        total_size = total_size.saturating_sub(size);
    }
    Ok(())
}

pub async fn usage_all(caches: &[&AudioCache]) -> Result<CacheUsage> {
    let mut usage = CacheUsage::default();
    for cache in caches {
        let mut entries = match fs::read_dir(&cache.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("キャッシュディレクトリを確認できません"),
        };
        while let Some(entry) = entries.next_entry().await? {
            if !is_audio_file(&entry.path()) {
                continue;
            }
            let metadata = match entry.metadata().await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("キャッシュファイルを確認できません"),
            };
            if metadata.is_file() {
                usage.file_count = usage.file_count.saturating_add(1);
                usage.used_bytes = usage.used_bytes.saturating_add(metadata.len());
            }
        }
    }
    Ok(usage)
}

pub async fn clear_all(caches: &[&AudioCache]) -> Result<()> {
    for cache in caches {
        cache.clear().await?;
    }
    Ok(())
}

fn is_audio_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("ogg")
}

fn is_temporary_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("tmp")
}

fn is_legacy_audio_file(path: &Path) -> bool {
    is_audio_file(path)
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(is_legacy_audio_id)
}

fn is_legacy_temporary_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    name.strip_suffix(".ogg.tmp")
        .is_some_and(is_legacy_audio_id)
}

fn is_legacy_audio_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_valid_engine_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("ファイル {path:?} を削除できません")),
    }
}

async fn write_atomic_state(
    destination: &Path,
    temporary: &Path,
    bytes: &[u8],
    label: &str,
) -> Result<()> {
    remove_if_exists(temporary).await?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)
        .await
        .with_context(|| format!("一時{label}を作成できません"))?;
    let write_result = async {
        file.write_all(bytes)
            .await
            .with_context(|| format!("一時{label}を書き込めません"))?;
        file.flush()
            .await
            .with_context(|| format!("一時{label}を書き込めません"))?;
        file.sync_all()
            .await
            .with_context(|| format!("一時{label}を同期できません"))?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    drop(file);
    if let Err(error) = write_result {
        let _ = remove_if_exists(temporary).await;
        return Err(error);
    }

    let result = (|| -> Result<()> {
        replace_file(temporary, destination).with_context(|| format!("{label}を確定できません"))?;
        if let Some(directory) = destination.parent() {
            if let Err(error) = sync_directory(directory) {
                eprintln!("{label}更新後のディレクトリ同期に失敗しました: {error:#}");
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = remove_if_exists(temporary).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use tokio::{fs, time::sleep};

    use super::{
        cleanup_all, prepare_cache_root, usage_all, AudioCache, CacheSignature, STATE_FILE,
    };
    use crate::config::AudioCodec;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn 削除されたキャッシュディレクトリを再作成する() {
        let (cache, directory) = test_cache();

        cache.prepare().await.unwrap();
        fs::remove_dir_all(&directory).await.unwrap();
        cache.ensure_ready().await.unwrap();

        assert!(directory.is_dir());
        assert!(directory.join(STATE_FILE).is_file());
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 記録済みの削除されたengineだけを削除する() {
        let unique = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "tts-server-cache-root-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("active")).await.unwrap();
        fs::create_dir_all(root.join("removed")).await.unwrap();
        fs::create_dir_all(root.join("unknown")).await.unwrap();

        prepare_cache_root(&root, &["active".to_owned(), "removed".to_owned()])
            .await
            .unwrap();
        AudioCache::new(
            root.join("removed"),
            7,
            CacheSignature {
                engine_url: "http://127.0.0.1:10101".to_owned(),
                cache_revision: 1,
                codec: AudioCodec::Opus,
                bitrate_kbps: 48,
            },
        )
        .prepare()
        .await
        .unwrap();

        prepare_cache_root(&root, &["active".to_owned()])
            .await
            .unwrap();

        assert!(root.join("active").is_dir());
        assert!(!root.join("removed").exists());
        assert!(root.join("unknown").is_dir());
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn 旧直下キャッシュはaudio_id形式だけを削除する() {
        let unique = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "tts-server-legacy-cache-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).await.unwrap();
        let audio_id = "a".repeat(64);
        fs::write(root.join(format!("{audio_id}.ogg")), b"legacy")
            .await
            .unwrap();
        fs::write(root.join(format!("{audio_id}.ogg.tmp")), b"temporary")
            .await
            .unwrap();
        fs::write(root.join("recording.ogg"), b"user file")
            .await
            .unwrap();

        prepare_cache_root(&root, &["active".to_owned()])
            .await
            .unwrap();

        assert!(!root.join(format!("{audio_id}.ogg")).exists());
        assert!(!root.join(format!("{audio_id}.ogg.tmp")).exists());
        assert!(root.join("recording.ogg").is_file());
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn 管理状態のない既存engineディレクトリは使用しない() {
        let (cache, directory) = test_cache();
        fs::create_dir_all(&directory).await.unwrap();
        let user_file = directory.join("user-data.txt");
        fs::write(&user_file, b"keep").await.unwrap();

        let error = cache.prepare().await.unwrap_err();

        assert!(error.to_string().contains("管理状態のない既存ディレクトリ"));
        assert_eq!(fs::read(&user_file).await.unwrap(), b"keep");
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 不正なengineキャッシュ状態を上書きしない() {
        let (cache, directory) = test_cache();
        fs::create_dir_all(&directory).await.unwrap();
        let state = directory.join(STATE_FILE);
        fs::write(&state, b"not json").await.unwrap();

        let error = cache.prepare().await.unwrap_err();

        assert!(error.to_string().contains("キャッシュ状態が不正"));
        assert_eq!(fs::read(&state).await.unwrap(), b"not json");
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 容量超過時は古い音声から削除する() {
        let (cache, directory) = test_cache();
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

        let preserved = cache.audio_path("new");
        cleanup_all(&[&cache], 1024 * 1024, Some(&preserved))
            .await
            .unwrap();

        assert!(!cache.audio_path("old").is_file());
        assert!(cache.audio_path("middle").is_file());
        assert!(cache.audio_path("new").is_file());
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 容量上限は全engineの合計へ適用する() {
        let (first, first_directory) = test_cache();
        let (second, second_directory) = test_cache();
        first.prepare().await.unwrap();
        second.prepare().await.unwrap();

        fs::write(first.audio_path("old"), vec![0; 700 * 1024])
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;
        fs::write(second.audio_path("new"), vec![0; 700 * 1024])
            .await
            .unwrap();

        let preserved = second.audio_path("new");
        cleanup_all(&[&first, &second], 1024 * 1024, Some(&preserved))
            .await
            .unwrap();

        assert!(!first.audio_path("old").is_file());
        assert!(second.audio_path("new").is_file());
        fs::remove_dir_all(first_directory).await.unwrap();
        fs::remove_dir_all(second_directory).await.unwrap();
    }

    #[tokio::test]
    async fn 使用量にはoggだけを含める() {
        let (cache, directory) = test_cache();
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

        let usage = usage_all(&[&cache]).await.unwrap();

        assert_eq!(usage.file_count, 2);
        assert_eq!(usage.used_bytes, 350);
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn 存在しないキャッシュディレクトリの使用量は0になる() {
        let (cache, directory) = test_cache();

        assert_eq!(usage_all(&[&cache]).await.unwrap(), Default::default());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn キャッシュ削除後も状態ファイルを維持する() {
        let (cache, directory) = test_cache();
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

    fn test_cache() -> (AudioCache, PathBuf) {
        let unique = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
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
        (cache, directory)
    }
}
