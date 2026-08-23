use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use reqwest::{header::CONTENT_TYPE, Client};
use serde::Deserialize;

#[derive(Clone)]
pub struct EngineClient {
    client: Client,
    base_url: String,
}

#[derive(Clone, Debug)]
pub struct SpeakerMeta {
    pub license: String,
}

#[derive(Debug, Deserialize)]
struct Speaker {
    speaker_uuid: String,
    styles: Vec<Style>,
}

#[derive(Debug, Deserialize)]
struct Style {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct SpeakerInfo {
    policy: Option<String>,
}

impl EngineClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    pub async fn load_speakers(&self) -> Result<HashMap<String, SpeakerMeta>> {
        let speakers: Vec<Speaker> = self
            .client
            .get(format!("{}/speakers", self.base_url))
            .send()
            .await
            .context("Engine の /speakers に接続できません")?
            .error_for_status()
            .context("Engine の /speakers がエラーを返しました")?
            .json()
            .await
            .context("Engine の話者一覧を解析できません")?;

        let mut licenses: HashMap<String, String> = HashMap::new();
        let mut result = HashMap::new();

        for speaker in speakers {
            let license = if let Some(license) = licenses.get(&speaker.speaker_uuid) {
                license.clone()
            } else {
                let license = self.load_license(&speaker.speaker_uuid).await?;
                licenses.insert(speaker.speaker_uuid.clone(), license.clone());
                license
            };

            for style in speaker.styles {
                result.insert(
                    style.id.to_string(),
                    SpeakerMeta {
                        license: license.clone(),
                    },
                );
            }
        }

        if result.is_empty() {
            bail!("Engine から利用可能な話者IDを取得できませんでした");
        }

        Ok(result)
    }

    async fn load_license(&self, speaker_uuid: &str) -> Result<String> {
        let info: SpeakerInfo = self
            .client
            .get(format!("{}/speaker_info", self.base_url))
            .query(&[("speaker_uuid", speaker_uuid), ("resource_format", "url")])
            .send()
            .await
            .with_context(|| format!("話者 {speaker_uuid} の情報を取得できません"))?
            .error_for_status()
            .with_context(|| format!("話者 {speaker_uuid} の情報取得に失敗しました"))?
            .json()
            .await
            .with_context(|| format!("話者 {speaker_uuid} の情報を解析できません"))?;

        Ok(info
            .policy
            .as_deref()
            .and_then(extract_license_heading)
            .unwrap_or("Unknown")
            .to_owned())
    }

    pub async fn synthesize(&self, speaker_id: &str, text: &str) -> Result<Vec<u8>> {
        let query = self
            .client
            .post(format!("{}/audio_query", self.base_url))
            .query(&[("text", text), ("speaker", speaker_id)])
            .send()
            .await
            .context("Engine の audio_query に接続できません")?
            .error_for_status()
            .context("Engine の audio_query がエラーを返しました")?
            .bytes()
            .await
            .context("Engine の audio_query 応答を読み込めません")?;

        let wav = self
            .client
            .post(format!("{}/synthesis", self.base_url))
            .query(&[("speaker", speaker_id)])
            .header(CONTENT_TYPE, "application/json")
            .body(query)
            .send()
            .await
            .context("Engine の synthesis に接続できません")?
            .error_for_status()
            .context("Engine の synthesis がエラーを返しました")?
            .bytes()
            .await
            .context("Engine の WAV 応答を読み込めません")?;

        Ok(wav.to_vec())
    }
}

fn extract_license_heading(policy: &str) -> Option<&str> {
    policy.lines().find_map(|line| {
        let line = line.trim();
        if !line.starts_with('#') {
            return None;
        }
        let heading = line.trim_start_matches('#').trim();
        (!heading.is_empty()).then_some(heading)
    })
}

#[cfg(test)]
mod tests {
    use super::extract_license_heading;

    #[test]
    fn markdownの最初の見出しを取得する() {
        let policy = "\n説明\n## Aivis Common Model License 1.0\n本文";
        assert_eq!(
            extract_license_heading(policy),
            Some("Aivis Common Model License 1.0")
        );
    }

    #[test]
    fn 見出しがなければ取得しない() {
        assert_eq!(extract_license_heading("本文だけです"), None);
    }
}
