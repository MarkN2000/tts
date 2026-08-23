use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub listen: String,
    pub engine_url: String,
    pub public_base_url: String,
    pub api_revision: String,
    pub default_id: String,
    pub cache_dir: PathBuf,
    pub cache_days: u64,
    pub cache_max_mb: u64,
    pub cache_revision: u64,
    pub ffmpeg_path: PathBuf,
    pub codec: AudioCodec,
    pub bitrate_kbps: u32,
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
        let source = fs::read_to_string(path)
            .with_context(|| format!("設定ファイル {path:?} を読み込めません"))?;
        let mut config: Self =
            toml::from_str(&source).with_context(|| format!("設定ファイル {path:?} が不正です"))?;

        config.engine_url = config.engine_url.trim_end_matches('/').to_owned();
        config.public_base_url = config.public_base_url.trim_end_matches('/').to_owned();
        if config.cache_dir.is_relative() {
            let base_directory = path.parent().unwrap_or_else(|| Path::new("."));
            config.cache_dir = base_directory.join(&config.cache_dir);
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.engine_url.is_empty() {
            bail!("engine_url は空にできません");
        }
        if self.public_base_url.is_empty() {
            bail!("public_base_url は空にできません");
        }
        if self.default_id.is_empty() {
            bail!("default_id は空にできません");
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

        reqwest::Url::parse(&self.engine_url).context("engine_url がURLとして不正です")?;
        reqwest::Url::parse(&self.public_base_url)
            .context("public_base_url がURLとして不正です")?;

        Ok(())
    }
}
