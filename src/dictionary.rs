use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserDictWordType {
    ProperNoun,
    CommonNoun,
    Verb,
    Adjective,
    Suffix,
}

impl UserDictWordType {
    pub fn as_engine_value(self) -> &'static str {
        match self {
            Self::ProperNoun => "PROPER_NOUN",
            Self::CommonNoun => "COMMON_NOUN",
            Self::Verb => "VERB",
            Self::Adjective => "ADJECTIVE",
            Self::Suffix => "SUFFIX",
        }
    }

    fn from_engine_value(value: &str) -> Option<Self> {
        match value {
            "PROPER_NOUN" => Some(Self::ProperNoun),
            "COMMON_NOUN" => Some(Self::CommonNoun),
            "VERB" => Some(Self::Verb),
            "ADJECTIVE" => Some(Self::Adjective),
            "SUFFIX" => Some(Self::Suffix),
            _ => None,
        }
    }

    fn from_context_id(context_id: u64) -> Option<Self> {
        match context_id {
            1348 => Some(Self::ProperNoun),
            1345 => Some(Self::CommonNoun),
            642 => Some(Self::Verb),
            20 => Some(Self::Adjective),
            1358 => Some(Self::Suffix),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserDictWordInput {
    pub surface: String,
    pub pronunciation: String,
    pub accent_type: u32,
    pub word_type: UserDictWordType,
    pub priority: u8,
}

impl UserDictWordInput {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.surface.trim().is_empty() {
            return Err("単語を入力してください");
        }
        if !is_valid_pronunciation(&self.pronunciation) {
            return Err("読みは全角カタカナで入力してください");
        }
        if self.priority > 10 {
            return Err("優先度は0から10で指定してください");
        }
        Ok(())
    }

    pub fn engine_query(&self) -> Vec<(&'static str, String)> {
        vec![
            ("surface", self.surface.clone()),
            ("pronunciation", self.pronunciation.clone()),
            ("accent_type", self.accent_type.to_string()),
            ("word_type", self.word_type.as_engine_value().to_owned()),
            ("priority", self.priority.to_string()),
        ]
    }
}

pub fn is_valid_pronunciation(pronunciation: &str) -> bool {
    !pronunciation.is_empty()
        && pronunciation
            .chars()
            .all(|character| matches!(character, '\u{30A1}'..='\u{30F4}' | '\u{30FC}'))
}

#[derive(Clone, Debug, Serialize)]
pub struct UserDictWord {
    pub uuid: String,
    #[serde(flatten)]
    pub input: UserDictWordInput,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserDictionary {
    pub words: Vec<UserDictWord>,
    pub has_excluded_words: bool,
}

pub fn parse_user_dictionary(response: Value) -> Result<UserDictionary> {
    let entries = response
        .as_object()
        .ok_or_else(|| anyhow!("Engine の user_dict 応答がオブジェクトではありません"))?;
    let mut words = entries
        .iter()
        .filter_map(|(uuid, value)| parse_word(uuid, value))
        .collect::<Vec<_>>();
    words.sort_by(|left, right| {
        left.input
            .surface
            .cmp(&right.input.surface)
            .then_with(|| left.uuid.cmp(&right.uuid))
    });
    Ok(UserDictionary {
        has_excluded_words: words.len() != entries.len(),
        words,
    })
}

fn parse_word(uuid: &str, value: &Value) -> Option<UserDictWord> {
    let object = value.as_object()?;
    let surface = single_string(object.get("surface")?)?;
    let pronunciation = single_string(object.get("pronunciation")?)?;
    let accent_type = u32::try_from(single_u64(object.get("accent_type")?)?).ok()?;
    let priority = u8::try_from(object.get("priority")?.as_u64()?).ok()?;
    let word_type = match object.get("word_type") {
        Some(Value::String(value)) => UserDictWordType::from_engine_value(value)?,
        Some(_) => return None,
        None => UserDictWordType::from_context_id(object.get("context_id")?.as_u64()?)?,
    };
    Some(UserDictWord {
        uuid: uuid.to_owned(),
        input: UserDictWordInput {
            surface,
            pronunciation,
            accent_type,
            word_type,
            priority,
        },
    })
}

fn single_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) if values.len() == 1 => values[0].as_str().map(ToOwned::to_owned),
        _ => None,
    }
}

fn single_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value.as_u64(),
        Value::Array(values) if values.len() == 1 => values[0].as_u64(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_user_dictionary, UserDictWordInput, UserDictWordType};

    #[test]
    fn 共通単一語だけを辞書一覧へ残す() {
        let dictionary = parse_user_dictionary(json!({
            "00000000-0000-0000-0000-000000000001": {
                "surface": "単語",
                "pronunciation": "タンゴ",
                "accent_type": 1,
                "word_type": "PROPER_NOUN",
                "priority": 5
            },
            "00000000-0000-0000-0000-000000000002": {
                "surface": ["複合", "語"],
                "pronunciation": ["フクゴウ", "ゴ"],
                "accent_type": [1, 0],
                "word_type": "PROPER_NOUN",
                "priority": 5
            }
        }))
        .unwrap();

        assert_eq!(dictionary.words.len(), 1);
        assert_eq!(dictionary.words[0].input.surface, "単語");
        assert!(dictionary.has_excluded_words);
    }

    #[test]
    fn 辞書入力を検証する() {
        let valid = UserDictWordInput {
            surface: "単語".to_owned(),
            pronunciation: "タンゴ".to_owned(),
            accent_type: 1,
            word_type: UserDictWordType::ProperNoun,
            priority: 5,
        };
        assert!(valid.validate().is_ok());

        let invalid = UserDictWordInput {
            pronunciation: "tango".to_owned(),
            ..valid
        };
        assert!(invalid.validate().is_err());
    }
}
