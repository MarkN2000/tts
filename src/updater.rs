use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    process::Command,
    sync::Mutex,
    time::{timeout, Duration},
};

const RELEASE_API_URL: &str = "https://api.github.com/repos/MarkN2000/tts/releases/latest";
const RELEASE_ASSET_NAME: &str = "tts-server-linux-x86_64";
const MAX_BINARY_SIZE: u64 = 64 * 1024 * 1024;

pub(crate) struct UpdateManager {
    client: Client,
    executable: PathBuf,
    apply_state: Mutex<bool>,
}

#[derive(Serialize)]
pub(crate) struct VersionInfo {
    supported: bool,
    current_version: &'static str,
}

#[derive(Serialize)]
pub(crate) struct UpdateInfo {
    supported: bool,
    current_version: &'static str,
    latest_version: Option<String>,
    update_available: bool,
}

#[derive(Serialize)]
pub(crate) struct AppliedUpdate {
    version: String,
}

pub(crate) enum ApplyError {
    NoUpdate,
    Restarting,
    Failed(anyhow::Error),
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Clone, Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

struct AvailableUpdate {
    version: Version,
    asset: GitHubAsset,
}

impl UpdateManager {
    pub(crate) fn new(executable: PathBuf) -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("tts-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("アップデート確認用HTTPクライアントを作成できません")?;
        Ok(Self {
            client,
            executable,
            apply_state: Mutex::new(false),
        })
    }

    pub(crate) fn version_info(&self) -> VersionInfo {
        VersionInfo {
            supported: is_supported(),
            current_version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub(crate) async fn check(&self) -> Result<UpdateInfo> {
        if !is_supported() {
            return Ok(UpdateInfo {
                supported: false,
                current_version: env!("CARGO_PKG_VERSION"),
                latest_version: None,
                update_available: false,
            });
        }

        let release = self.latest_release().await?;
        let latest = parse_release_version(&release)?;
        let update_available = latest > current_version()?;
        if update_available {
            let asset = find_asset(&release)?;
            validate_asset_metadata(asset)?;
        }
        Ok(UpdateInfo {
            supported: true,
            current_version: env!("CARGO_PKG_VERSION"),
            latest_version: Some(latest.to_string()),
            update_available,
        })
    }

    pub(crate) async fn apply(&self) -> Result<AppliedUpdate, ApplyError> {
        if !is_supported() {
            return Err(ApplyError::Failed(anyhow!(
                "アップデートは Linux x86_64 だけに対応しています"
            )));
        }

        let mut applied = self
            .apply_state
            .try_lock()
            .map_err(|_| ApplyError::Failed(anyhow!("別のアップデート処理が進行中です")))?;
        if *applied {
            return Err(ApplyError::Restarting);
        }
        let update = self
            .available_update()
            .await
            .map_err(ApplyError::Failed)?
            .ok_or(ApplyError::NoUpdate)?;
        self.download_and_replace(&update)
            .await
            .map_err(ApplyError::Failed)?;
        *applied = true;
        Ok(AppliedUpdate {
            version: update.version.to_string(),
        })
    }

    async fn latest_release(&self) -> Result<GitHubRelease> {
        self.client
            .get(RELEASE_API_URL)
            .send()
            .await
            .context("GitHubへ接続できません")?
            .error_for_status()
            .context("GitHubの最新リリースを取得できません")?
            .json()
            .await
            .context("GitHubのリリース情報を読み取れません")
    }

    async fn available_update(&self) -> Result<Option<AvailableUpdate>> {
        let release = self.latest_release().await?;
        let version = parse_release_version(&release)?;
        if version <= current_version()? {
            return Ok(None);
        }
        let asset = find_asset(&release)?.clone();
        validate_asset_metadata(&asset)?;
        Ok(Some(AvailableUpdate { version, asset }))
    }

    async fn download_and_replace(&self, update: &AvailableUpdate) -> Result<()> {
        let parent = self
            .executable
            .parent()
            .context("実行ファイルのディレクトリを取得できません")?;
        let file_name = self
            .executable
            .file_name()
            .and_then(|name| name.to_str())
            .context("実行ファイル名を取得できません")?;
        let stage = parent.join(format!(".{file_name}.update.part"));
        let previous_part = parent.join(format!(".{file_name}.previous.part"));
        let previous = parent.join(format!("{file_name}.previous"));

        remove_if_exists(&stage).await?;
        remove_if_exists(&previous_part).await?;
        let result = self
            .download_and_replace_inner(update, &stage, &previous_part, &previous)
            .await;
        if result.is_err() {
            let _ = remove_if_exists(&stage).await;
            let _ = remove_if_exists(&previous_part).await;
        }
        result
    }

    async fn download_and_replace_inner(
        &self,
        update: &AvailableUpdate,
        stage: &Path,
        previous_part: &Path,
        previous: &Path,
    ) -> Result<()> {
        let mut response = self
            .client
            .get(&update.asset.browser_download_url)
            .send()
            .await
            .context("更新ファイルをダウンロードできません")?
            .error_for_status()
            .context("更新ファイルをダウンロードできません")?;
        if response
            .content_length()
            .is_some_and(|size| size != update.asset.size)
        {
            bail!("更新ファイルのサイズがリリース情報と一致しません");
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage)
            .await
            .with_context(|| format!("{}へ更新ファイルを保存できません", stage.display()))?;
        let mut size = 0_u64;
        let mut hasher = Sha256::new();
        let mut header = Vec::with_capacity(20);
        while let Some(chunk) = response
            .chunk()
            .await
            .context("更新ファイルを読み取れません")?
        {
            size += chunk.len() as u64;
            if size > update.asset.size || size > MAX_BINARY_SIZE {
                bail!("更新ファイルのサイズがリリース情報と一致しません");
            }
            if header.len() < 20 {
                let remaining = 20 - header.len();
                header.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .context("更新ファイルを保存できません")?;
        }
        if size != update.asset.size {
            bail!("更新ファイルのサイズがリリース情報と一致しません");
        }
        validate_elf_header(&header)?;
        validate_digest(&hasher.finalize(), &update.asset)?;
        set_executable(stage).await?;
        file.flush().await.context("更新ファイルを保存できません")?;
        file.sync_all()
            .await
            .context("更新ファイルをディスクへ同期できません")?;
        drop(file);

        verify_embedded_version(stage, &update.version).await?;

        fs::copy(&self.executable, previous_part)
            .await
            .context("現在の実行ファイルをバックアップできません")?;
        sync_file(previous_part).await?;
        fs::rename(previous_part, previous)
            .await
            .context("以前の実行ファイルを保存できません")?;
        sync_directory(
            self.executable
                .parent()
                .context("実行ファイルのディレクトリを取得できません")?,
        )?;

        fs::rename(stage, &self.executable)
            .await
            .context("実行ファイルを置換できません")?;
        if let Err(error) = sync_directory(
            self.executable
                .parent()
                .context("実行ファイルのディレクトリを取得できません")?,
        ) {
            eprintln!("更新後のディレクトリ同期に失敗しました: {error:#}");
        }
        Ok(())
    }
}

fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).context("現在のバージョンがSemVerではありません")
}

fn parse_release_version(release: &GitHubRelease) -> Result<Version> {
    if release.draft || release.prerelease {
        bail!("最新リリースが正式リリースではありません");
    }
    let tag = release
        .tag_name
        .strip_prefix('v')
        .context("最新リリースのタグが v で始まっていません")?;
    Version::parse(tag).context("最新リリースのタグがSemVerではありません")
}

fn find_asset(release: &GitHubRelease) -> Result<&GitHubAsset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == RELEASE_ASSET_NAME)
        .context("最新リリースにLinux自己更新用ファイルがありません")
}

fn validate_asset_metadata(asset: &GitHubAsset) -> Result<()> {
    if asset.size == 0 || asset.size > MAX_BINARY_SIZE {
        bail!("更新ファイルのサイズが許容範囲外です");
    }
    let digest = asset
        .digest
        .as_deref()
        .context("更新ファイルにSHA-256 digestがありません")?;
    let hash = digest
        .strip_prefix("sha256:")
        .context("更新ファイルのdigest形式が不正です")?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("更新ファイルのSHA-256 digestが不正です");
    }
    Ok(())
}

fn validate_elf_header(header: &[u8]) -> Result<()> {
    if header.len() < 20
        || &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || u16::from_le_bytes([header[18], header[19]]) != 0x3e
    {
        bail!("更新ファイルがLinux x86_64の実行ファイルではありません");
    }
    Ok(())
}

fn validate_digest(actual: &[u8], asset: &GitHubAsset) -> Result<()> {
    let expected = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .context("更新ファイルにSHA-256 digestがありません")?;
    let actual = actual
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("更新ファイルのSHA-256が一致しません");
    }
    Ok(())
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("{}を削除できません", path.display())),
    }
}

async fn sync_file(path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .with_context(|| format!("{}を開けません", path.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("{}をディスクへ同期できません", path.display()))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
async fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .await
        .context("更新ファイルへ実行権限を設定できません")
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
async fn set_executable(_path: &Path) -> Result<()> {
    bail!("アップデートは Linux x86_64 だけに対応しています")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("{}をディスクへ同期できません", path.display()))
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn sync_directory(_path: &Path) -> Result<()> {
    bail!("アップデートは Linux x86_64 だけに対応しています")
}

async fn verify_embedded_version(path: &Path, expected: &Version) -> Result<()> {
    let mut command = Command::new(path);
    command.arg("--version").kill_on_drop(true);
    let output = timeout(Duration::from_secs(10), command.output())
        .await
        .context("更新ファイルの事前確認が10秒以内に完了しませんでした")?
        .context("更新ファイルを事前確認できません")?;
    if !output.status.success() {
        bail!("更新ファイルの事前確認に失敗しました");
    }
    let stdout = String::from_utf8(output.stdout).context("更新ファイルのバージョンが不正です")?;
    if stdout.trim() != format!("tts-server {expected}") {
        bail!("更新ファイルのバージョンがリリース情報と一致しません");
    }
    Ok(())
}

const fn is_supported() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{
        parse_release_version, validate_asset_metadata, validate_digest, validate_elf_header,
        GitHubAsset, GitHubRelease, RELEASE_ASSET_NAME,
    };

    fn release(tag: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        }
    }

    #[test]
    fn release_tagをsemverとして解釈する() {
        assert_eq!(
            parse_release_version(&release("v0.10.0"))
                .unwrap()
                .to_string(),
            "0.10.0"
        );
        assert!(parse_release_version(&release("0.10.0")).is_err());
        assert!(parse_release_version(&release("vbad")).is_err());
    }

    #[test]
    fn draftとprereleaseを拒否する() {
        let mut draft = release("v1.0.0");
        draft.draft = true;
        assert!(parse_release_version(&draft).is_err());
        let mut prerelease = release("v1.0.0");
        prerelease.prerelease = true;
        assert!(parse_release_version(&prerelease).is_err());
    }

    #[test]
    fn assetのサイズとdigestを検証する() {
        let valid = GitHubAsset {
            name: RELEASE_ASSET_NAME.to_owned(),
            size: 1024,
            digest: Some(format!("sha256:{}", "a".repeat(64))),
            browser_download_url: "https://example.invalid/binary".to_owned(),
        };
        assert!(validate_asset_metadata(&valid).is_ok());
        let mut invalid = valid.clone();
        invalid.digest = None;
        assert!(validate_asset_metadata(&invalid).is_err());
        invalid = valid;
        invalid.size = 65 * 1024 * 1024;
        assert!(validate_asset_metadata(&invalid).is_err());
    }

    #[test]
    fn linux_x86_64のelfだけを許可する() {
        let mut header = [0_u8; 20];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[5] = 1;
        header[18..20].copy_from_slice(&0x3e_u16.to_le_bytes());
        assert!(validate_elf_header(&header).is_ok());

        header[18..20].copy_from_slice(&0xb7_u16.to_le_bytes());
        assert!(validate_elf_header(&header).is_err());
    }

    #[test]
    fn ダウンロード内容のsha256を検証する() {
        let bytes = b"binary";
        let digest = Sha256::digest(bytes);
        let asset = GitHubAsset {
            name: RELEASE_ASSET_NAME.to_owned(),
            size: bytes.len() as u64,
            digest: Some(format!(
                "sha256:{}",
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )),
            browser_download_url: "https://example.invalid/binary".to_owned(),
        };
        assert!(validate_digest(&digest, &asset).is_ok());

        let different = Sha256::digest(b"different");
        assert!(validate_digest(&different, &asset).is_err());
    }
}
