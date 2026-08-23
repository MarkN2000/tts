use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{header::CONTENT_TYPE, Client};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::dictionary::{parse_user_dictionary, UserDictWordInput, UserDictionary};

#[derive(Clone)]
pub struct EngineClient {
    client: Client,
    base_url: String,
}

#[derive(Clone, Debug)]
pub struct SpeakerMeta {
    pub license: String,
}

pub struct SpeakerCatalog {
    pub speakers: HashMap<String, SpeakerMeta>,
    pub raw_json: Vec<u8>,
}

#[derive(Debug)]
pub enum UserDictPreviewError {
    InvalidInput,
    Engine(anyhow::Error),
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

    pub async fn load_speakers(&self) -> Result<SpeakerCatalog> {
        let raw_json = self
            .client
            .get(format!("{}/speakers", self.base_url))
            .send()
            .await
            .context("Engine の /speakers に接続できません")?
            .error_for_status()
            .context("Engine の /speakers がエラーを返しました")?
            .bytes()
            .await
            .context("Engine の話者一覧を読み込めません")?
            .to_vec();
        let speakers: Vec<Speaker> =
            serde_json::from_slice(&raw_json).context("Engine の話者一覧を解析できません")?;

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

        Ok(SpeakerCatalog {
            speakers: result,
            raw_json,
        })
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
        let query = self.create_audio_query(speaker_id, text).await?;
        self.synthesize_audio_query(speaker_id, &query).await
    }

    async fn create_audio_query(&self, speaker_id: &str, text: &str) -> Result<Value> {
        self.client
            .post(format!("{}/audio_query", self.base_url))
            .query(&[("text", text), ("speaker", speaker_id)])
            .send()
            .await
            .context("Engine の audio_query に接続できません")?
            .error_for_status()
            .context("Engine の audio_query がエラーを返しました")?
            .json()
            .await
            .context("Engine の audio_query 応答を解析できません")
    }

    async fn synthesize_audio_query(
        &self,
        speaker_id: &str,
        audio_query: &Value,
    ) -> Result<Vec<u8>> {
        let wav = self
            .client
            .post(format!("{}/synthesis", self.base_url))
            .query(&[("speaker", speaker_id)])
            .header(CONTENT_TYPE, "application/json")
            .json(audio_query)
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

    pub async fn synthesize_user_dict_preview(
        &self,
        speaker_id: &str,
        pronunciation: &str,
        accent_type: u32,
    ) -> std::result::Result<Vec<u8>, UserDictPreviewError> {
        let mut audio_query = self
            .create_audio_query(speaker_id, pronunciation)
            .await
            .map_err(UserDictPreviewError::Engine)?;
        apply_user_dict_preview_accent(&mut audio_query, pronunciation, accent_type)?;
        self.refresh_user_dict_preview_pitch(speaker_id, &mut audio_query)
            .await
            .map_err(UserDictPreviewError::Engine)?;
        self.synthesize_audio_query(speaker_id, &audio_query)
            .await
            .map_err(UserDictPreviewError::Engine)
    }

    async fn refresh_user_dict_preview_pitch(
        &self,
        speaker_id: &str,
        audio_query: &mut Value,
    ) -> Result<()> {
        let accent_phrases = audio_query
            .get("accent_phrases")
            .ok_or_else(|| anyhow!("試聴用のアクセント句がありません"))?;
        let refreshed: Value = self
            .client
            .post(format!("{}/mora_pitch", self.base_url))
            .query(&[("speaker", speaker_id)])
            .json(accent_phrases)
            .send()
            .await
            .context("Engine の mora_pitch に接続できません")?
            .error_for_status()
            .context("Engine の mora_pitch がエラーを返しました")?
            .json()
            .await
            .context("Engine の mora_pitch 応答を解析できません")?;
        validate_refreshed_preview_pitch(accent_phrases, &refreshed)?;
        audio_query["accent_phrases"] = refreshed;
        Ok(())
    }

    pub async fn load_user_dictionary(&self) -> Result<UserDictionary> {
        let response: Value = self
            .client
            .get(format!("{}/user_dict", self.base_url))
            .query(&[("enable_compound_accent", true)])
            .send()
            .await
            .context("Engine の user_dict に接続できません")?
            .error_for_status()
            .context("Engine の user_dict がエラーを返しました")?
            .json()
            .await
            .context("Engine の user_dict 応答を解析できません")?;

        parse_user_dictionary(response)
    }

    pub async fn add_user_dict_word(&self, word: &UserDictWordInput) -> Result<Uuid> {
        let uuid: Uuid = self
            .client
            .post(format!("{}/user_dict_word", self.base_url))
            .query(&word.engine_query())
            .send()
            .await
            .context("Engine の単語追加APIに接続できません")?
            .error_for_status()
            .context("Engine が単語の追加を拒否しました")?
            .json()
            .await
            .context("Engine の単語追加応答を解析できません")?;
        Ok(uuid)
    }

    pub async fn update_user_dict_word(&self, uuid: Uuid, word: &UserDictWordInput) -> Result<()> {
        self.client
            .put(format!("{}/user_dict_word/{uuid}", self.base_url))
            .query(&word.engine_query())
            .send()
            .await
            .context("Engine の単語更新APIに接続できません")?
            .error_for_status()
            .context("Engine が単語の更新を拒否しました")?;
        Ok(())
    }

    pub async fn delete_user_dict_word(&self, uuid: Uuid) -> Result<()> {
        self.client
            .delete(format!("{}/user_dict_word/{uuid}", self.base_url))
            .send()
            .await
            .context("Engine の単語削除APIに接続できません")?
            .error_for_status()
            .context("Engine が単語の削除を拒否しました")?;
        Ok(())
    }
}

fn apply_user_dict_preview_accent(
    audio_query: &mut Value,
    pronunciation: &str,
    accent_type: u32,
) -> std::result::Result<(), UserDictPreviewError> {
    let accent_phrases = audio_query
        .get("accent_phrases")
        .and_then(Value::as_array)
        .ok_or(UserDictPreviewError::InvalidInput)?;
    if accent_phrases.is_empty() {
        return Err(UserDictPreviewError::InvalidInput);
    }

    let mut moras = Vec::new();
    for accent_phrase in accent_phrases {
        let phrase_moras = accent_phrase
            .get("moras")
            .and_then(Value::as_array)
            .ok_or(UserDictPreviewError::InvalidInput)?;
        if phrase_moras.is_empty() {
            return Err(UserDictPreviewError::InvalidInput);
        }
        moras.extend(phrase_moras.iter().cloned());
        if !matches!(accent_phrase.get("pause_mora"), None | Some(Value::Null))
            || !matches!(
                accent_phrase.get("is_interrogative"),
                None | Some(Value::Bool(false))
            )
        {
            return Err(UserDictPreviewError::InvalidInput);
        }
    }

    let mora_count = moras.len();
    let generated_pronunciation = moras
        .iter()
        .map(|mora| mora.get("text")?.as_str())
        .collect::<Option<String>>()
        .ok_or(UserDictPreviewError::InvalidInput)?;
    if generated_pronunciation != pronunciation {
        return Err(UserDictPreviewError::InvalidInput);
    }
    let accent = if accent_type == 0 {
        mora_count
    } else {
        usize::try_from(accent_type).map_err(|_| UserDictPreviewError::InvalidInput)?
    };
    if !(1..=mora_count).contains(&accent) {
        return Err(UserDictPreviewError::InvalidInput);
    }

    let mut merged_phrase = accent_phrases[0].clone();
    merged_phrase["moras"] = Value::Array(moras);
    merged_phrase["accent"] = Value::from(accent);
    audio_query["accent_phrases"] = Value::Array(vec![merged_phrase]);
    Ok(())
}

fn validate_refreshed_preview_pitch(expected: &Value, refreshed: &Value) -> Result<()> {
    let expected_phrase = expected
        .as_array()
        .filter(|phrases| phrases.len() == 1)
        .and_then(|phrases| phrases.first())
        .ok_or_else(|| anyhow!("試聴用のアクセント句が1件ではありません"))?;
    let refreshed_phrase = refreshed
        .as_array()
        .filter(|phrases| phrases.len() == 1)
        .and_then(|phrases| phrases.first())
        .ok_or_else(|| anyhow!("mora_pitch のアクセント句が1件ではありません"))?;
    let mora_texts = |phrase: &Value| -> Option<Vec<String>> {
        phrase
            .get("moras")?
            .as_array()?
            .iter()
            .map(|mora| Some(mora.get("text")?.as_str()?.to_owned()))
            .collect()
    };
    if mora_texts(expected_phrase) != mora_texts(refreshed_phrase)
        || expected_phrase.get("accent") != refreshed_phrase.get("accent")
    {
        bail!("mora_pitch が試聴用のモーラ構造を変更しました");
    }
    Ok(())
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
    use serde_json::json;

    use super::{apply_user_dict_preview_accent, extract_license_heading, UserDictPreviewError};

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

    #[test]
    fn 辞書試聴ではアクセント句を一語へまとめる() {
        let mut query = json!({
            "accent_phrases": [
                { "moras": [{ "text": "タン" }], "accent": 1, "pause_mora": null },
                { "moras": [{ "text": "タン" }, { "text": "メン" }], "accent": 1, "pause_mora": null }
            ]
        });

        apply_user_dict_preview_accent(&mut query, "タンタンメン", 2).unwrap();

        let phrases = query["accent_phrases"].as_array().unwrap();
        assert_eq!(phrases.len(), 1);
        assert_eq!(phrases[0]["accent"], 2);
        assert_eq!(phrases[0]["moras"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn 辞書試聴はモーラ数を超えるアクセントを拒否する() {
        let mut query = json!({
            "accent_phrases": [{
                "moras": [{ "text": "タン" }, { "text": "ゴ" }],
                "accent": 1
            }]
        });

        assert!(matches!(
            apply_user_dict_preview_accent(&mut query, "タンゴ", 3),
            Err(UserDictPreviewError::InvalidInput)
        ));
    }

    #[test]
    fn 辞書試聴のアクセント0は最終モーラへ変換する() {
        let mut query = json!({
            "accent_phrases": [{
                "moras": [{ "text": "タン" }, { "text": "ゴ" }],
                "accent": 1
            }]
        });

        apply_user_dict_preview_accent(&mut query, "タンゴ", 0).unwrap();

        assert_eq!(query["accent_phrases"][0]["accent"], 2);
    }

    #[test]
    fn 辞書試聴は生成された読みとの不一致を拒否する() {
        let mut query = json!({
            "accent_phrases": [{
                "moras": [{ "text": "タン" }, { "text": "ゴ" }],
                "accent": 1
            }]
        });

        assert!(matches!(
            apply_user_dict_preview_accent(&mut query, "リンゴ", 1),
            Err(UserDictPreviewError::InvalidInput)
        ));
    }
}
