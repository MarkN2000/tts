use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use crate::atomic_file::{replace_file, sync_directory};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub listen: String,
    pub admin_listen: String,
    pub public_base_url: String,
    pub api_revision: String,
    pub engines: Vec<EngineConfig>,
    pub cache_dir: PathBuf,
    pub cache_days: u64,
    pub cache_max_mb: u64,
    pub cache_revision: u64,
    pub ffmpeg_path: PathBuf,
    pub codec: AudioCodec,
    pub bitrate_kbps: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EngineConfig {
    pub id: String,
    pub name: String,
    pub engine_url: String,
    pub default_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Opus,
    Vorbis,
}

impl AudioCodec {
    pub fn ffmpeg_encoder(self) -> &'static str {
        match self {
            Self::Opus => "libopus",
            Self::Vorbis => "libvorbis",
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let original_bytes =
            fs::read(path).with_context(|| format!("設定ファイル {path:?} を読み込めません"))?;
        let source = std::str::from_utf8(&original_bytes)
            .with_context(|| format!("設定ファイル {path:?} はUTF-8ではありません"))?;

        let migrated_source = legacy_migration_source(source)?;
        let candidate = migrated_source.as_deref().unwrap_or(source);
        let config = Self::from_source(path, candidate)?;
        if let Some(migrated_source) = migrated_source {
            write_migrated_config(path, &original_bytes, &migrated_source)?;
        }
        Ok(config)
    }

    fn from_source(path: &Path, source: &str) -> Result<Self> {
        let mut config: Self =
            toml::from_str(source).with_context(|| format!("設定ファイル {path:?} が不正です"))?;

        for engine in &mut config.engines {
            engine.engine_url = engine.engine_url.trim_end_matches('/').to_owned();
        }
        config.public_base_url = config.public_base_url.trim_end_matches('/').to_owned();
        if config.cache_dir.is_relative() {
            let base_directory = path.parent().unwrap_or_else(|| Path::new("."));
            config.cache_dir = base_directory.join(&config.cache_dir);
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.public_base_url.is_empty() {
            bail!("public_base_url は空にできません");
        }
        if self.cache_days == 0 || self.cache_days.checked_mul(86_400).is_none() {
            bail!("cache_days は1以上の有効な日数にしてください");
        }
        if self.cache_max_mb == 0 || self.cache_max_mb.checked_mul(1024 * 1024).is_none() {
            bail!("cache_max_mb は1以上の有効な容量にしてください");
        }
        if self.bitrate_kbps == 0 {
            bail!("bitrate_kbps は1以上にしてください");
        }
        if self.api_revision.is_empty()
            || !self
                .api_revision
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("api_revision は英数字、-、_ だけで指定してください");
        }

        reqwest::Url::parse(&self.public_base_url)
            .context("public_base_url がURLとして不正です")?;
        self.validate_engines()?;
        self.public_address()?;
        self.admin_address()?;

        Ok(())
    }

    pub fn admin_address(&self) -> Result<SocketAddr> {
        let address: SocketAddr = self
            .admin_listen
            .parse()
            .context("admin_listen がIPアドレスとポートとして不正です")?;
        if address.port() == 0 || !is_private_or_local(address.ip()) {
            bail!("admin_listen はプライベートまたはループバックのIPアドレスで指定してください");
        }
        Ok(address)
    }

    pub fn public_address(&self) -> Result<SocketAddr> {
        let address: SocketAddr = self
            .listen
            .parse()
            .context("listen がIPアドレスとポートとして不正です")?;
        if address.port() == 0 {
            bail!("listen のポート番号は1以上にしてください");
        }
        Ok(address)
    }

    fn validate_engines(&self) -> Result<()> {
        if self.engines.is_empty() {
            bail!("engines は1件以上指定してください");
        }

        let mut ids = HashSet::new();
        let mut engine_urls = HashSet::new();
        for engine in &self.engines {
            if engine.id.is_empty()
                || !engine.id.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                })
            {
                bail!("engines の id は小文字英数字とハイフンだけで指定してください");
            }
            if !ids.insert(&engine.id) {
                bail!("engines の id が重複しています: {}", engine.id);
            }
            if engine.name.is_empty() {
                bail!("engines の name は空にできません: {}", engine.id);
            }
            if engine.engine_url.is_empty() {
                bail!("engines の engine_url は空にできません: {}", engine.id);
            }
            if engine.default_id.is_empty() {
                bail!("engines の default_id は空にできません: {}", engine.id);
            }

            let url = reqwest::Url::parse(&engine.engine_url).with_context(|| {
                format!("engines の engine_url がURLとして不正です: {}", engine.id)
            })?;
            if !matches!(url.scheme(), "http" | "https") {
                bail!(
                    "engines の engine_url は HTTP または HTTPS URL を指定してください: {}",
                    engine.id
                );
            }
            if !engine_urls.insert(&engine.engine_url) {
                bail!(
                    "engines の engine_url が重複しています: {}",
                    engine.engine_url
                );
            }
        }

        Ok(())
    }
}

const VOICEVOX_EXAMPLE: &str = r#"# VOICEVOXも追加する場合は、VOICEVOXを起動してから次の5行のコメントを外してください。
# [[engines]]
# id = "voicevox"
# name = "VOICEVOX"
# engine_url = "http://127.0.0.1:50021"
# default_id = "3"
"#;

/// 旧形式を検出して新形式の候補を返す。新形式は変更せずに `None` を返す。
fn legacy_migration_source(source: &str) -> Result<Option<String>> {
    let mut document = DocumentMut::from_str(source).context("設定ファイルがTOMLとして不正です")?;
    let has_engines = document.get("engines").is_some();
    let has_engine_url = document.get("engine_url").is_some();
    let has_default_id = document.get("default_id").is_some();

    if has_engines && (has_engine_url || has_default_id) {
        bail!("旧形式の engine_url/default_id と engines を同時に指定できません。設定ファイルは変更していません");
    }
    if has_engine_url != has_default_id {
        bail!("旧形式の engine_url と default_id は両方指定してください。設定ファイルは変更していません");
    }
    if has_engines || !has_engine_url {
        return Ok(None);
    }

    document
        .get("engine_url")
        .and_then(Item::as_str)
        .context("旧形式の engine_url は文字列で指定してください")?;
    document
        .get("default_id")
        .and_then(Item::as_str)
        .context("旧形式の default_id は文字列で指定してください")?;
    let (engine_url_key, engine_url_item) = document
        .remove_entry("engine_url")
        .expect("engine_url の存在を確認済みです");
    let (default_id_key, default_id_item) = document
        .remove_entry("default_id")
        .expect("default_id の存在を確認済みです");
    let mut engine = Table::new();
    engine["id"] = value("aivisspeech");
    engine["name"] = value("AivisSpeech");
    engine.insert_formatted(&engine_url_key, engine_url_item);
    engine.insert_formatted(&default_id_key, default_id_item);
    let mut engines = ArrayOfTables::new();
    engines.push(engine);
    document["engines"] = Item::ArrayOfTables(engines);

    let mut migrated = document.to_string();
    if !migrated.ends_with('\n') {
        migrated.push('\n');
    }
    migrated.push('\n');
    migrated.push_str(VOICEVOX_EXAMPLE);
    Ok(Some(migrated))
}

fn write_migrated_config(path: &Path, original_bytes: &[u8], migrated_source: &str) -> Result<()> {
    let directory = path
        .parent()
        .context("設定ファイルのディレクトリを取得できません")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("設定ファイル名を取得できません")?;
    let backup = directory.join(format!("{file_name}.pre-engines"));

    if fs::read(path).with_context(|| format!("設定ファイル {path:?} を再確認できません"))?
        != original_bytes
    {
        bail!("設定ファイルが読み込み後に変更されたため、移行を中止しました");
    }

    let metadata = fs::metadata(path)
        .with_context(|| format!("設定ファイル {path:?} の権限を取得できません"))?;
    ensure_migration_backup(&backup, original_bytes, metadata.permissions())
        .with_context(|| format!("移行前の設定を {} へ保存できません", backup.display()))?;
    sync_directory(directory)?;

    let (temporary, mut temporary_file) = create_temporary_file(directory, file_name)?;
    let result = (|| -> Result<()> {
        temporary_file
            .write_all(migrated_source.as_bytes())
            .with_context(|| format!("移行後の設定を {} へ保存できません", temporary.display()))?;
        temporary_file
            .flush()
            .with_context(|| format!("移行後の設定を {} へ保存できません", temporary.display()))?;
        temporary_file
            .set_permissions(metadata.permissions())
            .with_context(|| {
                format!(
                    "移行後の設定の権限を引き継げません: {}",
                    temporary.display()
                )
            })?;
        temporary_file.sync_all().with_context(|| {
            format!(
                "移行後の設定をディスクへ同期できません: {}",
                temporary.display()
            )
        })?;
        drop(temporary_file);

        if fs::read(path).with_context(|| format!("設定ファイル {path:?} を再確認できません"))?
            != original_bytes
        {
            bail!("設定ファイルが読み込み後に変更されたため、移行を中止しました");
        }

        replace_file(&temporary, path)?;
        if let Err(error) = sync_directory(directory) {
            eprintln!("設定移行後のディレクトリ同期に失敗しました: {error:#}");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_migration_backup(
    path: &Path,
    bytes: &[u8],
    permissions: std::fs::Permissions,
) -> Result<()> {
    if path.exists() {
        if fs::read(path)? == bytes {
            return Ok(());
        }
        bail!("既存の移行前バックアップと現在の設定が一致しません");
    }

    let directory = path
        .parent()
        .context("移行前バックアップのディレクトリを取得できません")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("移行前バックアップのファイル名を取得できません")?;
    let (temporary, mut file) = create_temporary_file(directory, file_name)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.flush()?;
        file.set_permissions(permissions)?;
        file.sync_all()?;
        drop(file);

        match fs::hard_link(&temporary, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(path)? == bytes {
                    Ok(())
                } else {
                    bail!("既存の移行前バックアップと現在の設定が一致しません")
                }
            }
            Err(error) => Err(error).context("移行前バックアップを確定できません"),
        }
    })();
    let _ = fs::remove_file(&temporary);
    result
}

fn create_temporary_file(directory: &Path, file_name: &str) -> Result<(PathBuf, File)> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("現在時刻がUNIXエポックより前です")
        .as_nanos();
    for attempt in 0..100 {
        let path = directory.join(format!(
            ".{file_name}.engines-migration-{}-{unique}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("移行用の一時ファイルを作成できません: {}", path.display())
                });
            }
        }
    }
    bail!("移行用の一時ファイル名を確保できません")
}

fn is_private_or_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_private() || address.is_loopback(),
        IpAddr::V6(address) => address.is_unique_local() || address.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        net::IpAddr,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{is_private_or_local, AudioCodec, Config, EngineConfig};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn config(engines: Vec<EngineConfig>) -> Config {
        Config {
            listen: "127.0.0.1:8080".to_owned(),
            admin_listen: "127.0.0.1:8081".to_owned(),
            public_base_url: "https://tts.example.com".to_owned(),
            api_revision: "v2".to_owned(),
            engines,
            cache_dir: PathBuf::from("./cache"),
            cache_days: 31,
            cache_max_mb: 1024,
            cache_revision: 1,
            ffmpeg_path: PathBuf::from("ffmpeg"),
            codec: AudioCodec::Vorbis,
            bitrate_kbps: 48,
        }
    }

    fn engine(id: &str, engine_url: &str) -> EngineConfig {
        EngineConfig {
            id: id.to_owned(),
            name: "テストEngine".to_owned(),
            engine_url: engine_url.to_owned(),
            default_id: "1".to_owned(),
        }
    }

    fn テスト用ディレクトリ() -> PathBuf {
        std::env::temp_dir().join(format!(
            "tts-server-config-test-{}-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn 設定ファイルを読み込む(source: &str) -> anyhow::Result<Config> {
        let directory = テスト用ディレクトリ();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        fs::write(&path, source).unwrap();
        let result = Config::load(&path);
        fs::remove_dir_all(&directory).unwrap();
        result
    }

    fn 旧形式の設定() -> &'static str {
        r#"# 利用者が書いたコメントは維持する
listen = "127.0.0.1:8080"
admin_listen = "127.0.0.1:8081"
public_base_url = "https://tts.example.com"
api_revision = "v2"

# 既存の任意設定も維持する
custom_value = "keep"
# 接続先のコメントも維持する
engine_url = "http://127.0.0.1:10101"
# 既定話者のコメントも維持する
default_id = "1878365376"

cache_dir = "./cache"
cache_days = 31
cache_max_mb = 1024
cache_revision = 1
ffmpeg_path = "ffmpeg"
codec = "vorbis"
bitrate_kbps = 48
"#
    }

    fn 新形式の設定() -> &'static str {
        r#"listen = "127.0.0.1:8080"
admin_listen = "127.0.0.1:8081"
public_base_url = "https://tts.example.com"
api_revision = "v2"
cache_dir = "./cache"
cache_days = 31
cache_max_mb = 1024
cache_revision = 1
ffmpeg_path = "ffmpeg"
codec = "vorbis"
bitrate_kbps = 48

[[engines]]
id = "aivisspeech"
name = "AivisSpeech"
engine_url = "http://127.0.0.1:10101"
default_id = "1878365376"
"#
    }

    fn 読み込み後に後始末<T>(
        source: &str,
        verify: impl FnOnce(&Path, anyhow::Result<Config>) -> T,
    ) -> T {
        let directory = テスト用ディレクトリ();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        fs::write(&path, source).unwrap();
        let value = verify(&path, Config::load(&path));
        fs::remove_dir_all(&directory).unwrap();
        value
    }

    #[test]
    fn 管理画面はローカル用アドレスだけを許可する() {
        assert!(is_private_or_local(IpAddr::V4("10.0.0.1".parse().unwrap())));
        assert!(is_private_or_local(IpAddr::V4(
            "127.0.0.1".parse().unwrap()
        )));
        assert!(is_private_or_local(IpAddr::V6("fd00::1".parse().unwrap())));
        assert!(!is_private_or_local(IpAddr::V4("0.0.0.0".parse().unwrap())));
        assert!(!is_private_or_local(IpAddr::V4("8.8.8.8".parse().unwrap())));
    }

    #[test]
    fn 公開待受はipアドレスと有効なポートだけを許可する() {
        let mut config = config(vec![engine("aivisspeech", "http://127.0.0.1:10101")]);
        config.listen = "not-an-address".to_owned();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("listen"));

        config.listen = "127.0.0.1:0".to_owned();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("ポート番号"));

        config.listen = "0.0.0.0:8080".to_owned();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn 複数engine設定を検証できる() {
        let config = config(vec![
            engine("aivisspeech", "http://127.0.0.1:10101"),
            engine("voicevox", "https://127.0.0.1:50021"),
        ]);

        assert!(config.validate().is_ok());
    }

    #[test]
    fn engine_idの重複を拒否する() {
        let config = config(vec![
            engine("voicevox", "http://127.0.0.1:50021"),
            engine("voicevox", "http://127.0.0.1:50022"),
        ]);

        assert!(config.validate().unwrap_err().to_string().contains("重複"));
    }

    #[test]
    fn engine_urlの正規化後の重複を拒否する() {
        let result = 設定ファイルを読み込む(
            r#"
                listen = "127.0.0.1:8080"
                admin_listen = "127.0.0.1:8081"
                public_base_url = "https://tts.example.com"
                api_revision = "v2"
                cache_dir = "./cache"
                cache_days = 31
                cache_max_mb = 1024
                cache_revision = 1
                ffmpeg_path = "ffmpeg"
                codec = "vorbis"
                bitrate_kbps = 48

                [[engines]]
                id = "aivisspeech"
                name = "AivisSpeech"
                engine_url = "http://127.0.0.1:10101/"
                default_id = "1"

                [[engines]]
                id = "voicevox"
                name = "VOICEVOX"
                engine_url = "http://127.0.0.1:10101"
                default_id = "3"
            "#,
        );

        assert!(result.unwrap_err().to_string().contains("重複"));
    }

    #[test]
    fn engine_urlはhttpかhttpsだけを許可する() {
        let config = config(vec![engine("voicevox", "ftp://127.0.0.1:50021")]);

        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("HTTP または HTTPS"));
    }

    #[test]
    fn 旧形式をコメントと無関係設定を保ったまま移行する() {
        読み込み後に後始末(旧形式の設定(), |path, result| {
            let config = result.unwrap();
            assert_eq!(config.api_revision, "v2");
            assert_eq!(config.engines.len(), 1);
            assert_eq!(config.engines[0].id, "aivisspeech");
            assert_eq!(config.engines[0].name, "AivisSpeech");

            let migrated = fs::read_to_string(path).unwrap();
            assert!(migrated.contains("# 利用者が書いたコメントは維持する"));
            assert!(migrated.contains("custom_value = \"keep\""));
            assert!(migrated.contains("# 接続先のコメントも維持する"));
            assert!(migrated.contains("# 既定話者のコメントも維持する"));
            assert!(migrated.contains("# [[engines]]"));
            let document = migrated.parse::<toml_edit::DocumentMut>().unwrap();
            assert!(document.get("engine_url").is_none());
            assert!(document.get("default_id").is_none());
            assert_eq!(
                fs::read(path.with_file_name("config.toml.pre-engines")).unwrap(),
                旧形式の設定().as_bytes()
            );
        });
    }

    #[test]
    fn 新旧混在では設定を変更しない() {
        let source = format!(
            "engine_url = \"http://127.0.0.1:50021\"\n{}",
            新形式の設定()
        );
        読み込み後に後始末(&source, |path, result| {
            assert!(result.unwrap_err().to_string().contains("同時に指定"));
            assert_eq!(fs::read_to_string(path).unwrap(), source);
            assert!(!path.with_file_name("config.toml.pre-engines").exists());
        });
    }

    #[test]
    fn 旧形式の片方だけでは設定を変更しない() {
        let source = 旧形式の設定().replace("default_id = \"1878365376\"\n", "");
        読み込み後に後始末(&source, |path, result| {
            assert!(result.unwrap_err().to_string().contains("両方指定"));
            assert_eq!(fs::read_to_string(path).unwrap(), source);
            assert!(!path.with_file_name("config.toml.pre-engines").exists());
        });
    }

    #[test]
    fn 移行済み設定の再読み込みでは変更しない() {
        読み込み後に後始末(旧形式の設定(), |path, result| {
            result.unwrap();
            let migrated = fs::read(path).unwrap();
            Config::load(path).unwrap();
            assert_eq!(fs::read(path).unwrap(), migrated);
        });
    }

    #[test]
    fn 移行前バックアップが既にある場合は設定を変更しない() {
        let directory = テスト用ディレクトリ();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        let backup = directory.join("config.toml.pre-engines");
        fs::write(&path, 旧形式の設定()).unwrap();
        fs::write(&backup, "既存バックアップ").unwrap();
        let before = fs::read(&path).unwrap();

        assert!(Config::load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "既存バックアップ");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn 同じ移行前バックアップは中断された移行の再試行に使う() {
        let directory = テスト用ディレクトリ();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        let backup = directory.join("config.toml.pre-engines");
        fs::write(&path, 旧形式の設定()).unwrap();
        fs::write(&backup, 旧形式の設定()).unwrap();

        let config = Config::load(&path).unwrap();

        assert_eq!(config.api_revision, "v2");
        assert_eq!(fs::read_to_string(&backup).unwrap(), 旧形式の設定());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn 移行後の候補が不正なら元の設定を保持する() {
        let source = 旧形式の設定().replace(
            "public_base_url = \"https://tts.example.com\"",
            "public_base_url = \"not a url\"",
        );
        読み込み後に後始末(&source, |path, result| {
            assert!(result.unwrap_err().to_string().contains("public_base_url"));
            assert_eq!(fs::read_to_string(path).unwrap(), source);
            assert!(!path.with_file_name("config.toml.pre-engines").exists());
        });
    }
}
