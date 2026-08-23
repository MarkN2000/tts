use std::{path::PathBuf, process::Stdio};

use anyhow::{bail, Context, Result};
use tokio::{fs, io::AsyncWriteExt, process::Command};

use crate::{cache::AudioCache, config::AudioCodec};

#[derive(Clone)]
pub struct FfmpegConverter {
    executable: PathBuf,
    codec: AudioCodec,
    bitrate_kbps: u32,
}

impl FfmpegConverter {
    pub fn new(executable: PathBuf, codec: AudioCodec, bitrate_kbps: u32) -> Self {
        Self {
            executable,
            codec,
            bitrate_kbps,
        }
    }

    pub async fn verify(&self) -> Result<()> {
        let status = Command::new(&self.executable)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .with_context(|| format!("FFmpeg {:?} を起動できません", self.executable))?;

        if !status.success() {
            bail!("FFmpeg の動作確認に失敗しました");
        }
        Ok(())
    }

    pub async fn convert(&self, wav: &[u8], cache: &AudioCache, audio_id: &str) -> Result<()> {
        let temporary_path = cache.temporary_path(audio_id);
        let output_path = cache.audio_path(audio_id);
        remove_if_exists(&temporary_path).await?;

        let mut child = Command::new(&self.executable)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i", "pipe:0"])
            .args(["-vn", "-c:a", self.codec.ffmpeg_encoder()])
            .args(["-b:a", &format!("{}k", self.bitrate_kbps)])
            .args(["-f", "ogg"])
            .arg(&temporary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("FFmpeg を起動できません")?;

        let mut stdin = child
            .stdin
            .take()
            .context("FFmpeg の標準入力を開けません")?;
        if let Err(error) = stdin.write_all(wav).await {
            let _ = child.kill().await;
            let _ = remove_if_exists(&temporary_path).await;
            return Err(error).context("WAV を FFmpeg に渡せません");
        }
        drop(stdin);

        let output = child
            .wait_with_output()
            .await
            .context("FFmpeg の完了を待機できません")?;
        if !output.status.success() {
            let _ = remove_if_exists(&temporary_path).await;
            let message = String::from_utf8_lossy(&output.stderr);
            bail!("FFmpeg の変換に失敗しました: {}", message.trim());
        }

        let metadata = fs::metadata(&temporary_path)
            .await
            .context("FFmpeg の出力ファイルを確認できません")?;
        if metadata.len() == 0 {
            remove_if_exists(&temporary_path).await?;
            bail!("FFmpeg が空の音声ファイルを出力しました");
        }

        remove_if_exists(&output_path).await?;
        fs::rename(&temporary_path, &output_path)
            .await
            .context("変換済み音声をキャッシュへ移動できません")
    }
}

async fn remove_if_exists(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("一時音声ファイルを削除できません"),
    }
}
