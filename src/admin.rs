use std::sync::Arc;

use anyhow::Error;
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{header::CACHE_CONTROL, header::CONTENT_TYPE, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    dictionary::{is_valid_pronunciation, UserDictWordInput},
    engine::UserDictPreviewError,
    AppState,
};

const PAGE: &str = include_str!("../web/dictionary.html");
const STYLES: &str = include_str!("../web/dictionary.css");
const SCRIPT: &str = include_str!("../web/dictionary.js");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/dictionary", get(page))
        .route("/dictionary.css", get(styles))
        .route("/dictionary.js", get(script))
        .route("/api/user-dict", get(load_dictionary))
        .route("/api/user-dict/preview", post(preview_word))
        .route("/api/user-dict/words", post(add_word))
        .route(
            "/api/user-dict/words/{word_uuid}",
            axum::routing::put(update_word).delete(delete_word),
        )
        .with_state(state)
}

async fn page() -> impl IntoResponse {
    static_response("text/html; charset=utf-8", PAGE)
}

async fn styles() -> impl IntoResponse {
    static_response("text/css; charset=utf-8", STYLES)
}

async fn script() -> impl IntoResponse {
    static_response("text/javascript; charset=utf-8", SCRIPT)
}

fn static_response(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-store")],
        body,
    )
}

async fn load_dictionary(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AdminError> {
    let dictionary = state
        .engine
        .load_user_dictionary()
        .await
        .map_err(AdminError::engine)?;
    Ok(Json(dictionary))
}

#[derive(Deserialize)]
struct PreviewRequest {
    pronunciation: String,
    accent_type: u32,
}

async fn preview_word(
    State(state): State<Arc<AppState>>,
    request: Result<Json<PreviewRequest>, JsonRejection>,
) -> Result<impl IntoResponse, AdminError> {
    let Json(request) = request.map_err(|_| AdminError::bad_request("JSONリクエストが不正です"))?;
    if !is_valid_pronunciation(&request.pronunciation) {
        return Err(AdminError::bad_request(
            "読みは全角カタカナで入力してください",
        ));
    }

    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    let wav = match state
        .engine
        .synthesize_user_dict_preview(
            &state.config.default_id,
            &request.pronunciation,
            request.accent_type,
        )
        .await
    {
        Ok(wav) => wav,
        Err(UserDictPreviewError::InvalidInput) => {
            return Err(AdminError::bad_request(
                "読みまたはアクセント位置を確認してください",
            ));
        }
        Err(UserDictPreviewError::Engine(error)) => return Err(AdminError::engine(error)),
    };

    Ok((
        [(CONTENT_TYPE, "audio/wav"), (CACHE_CONTROL, "no-store")],
        wav,
    ))
}

#[derive(Serialize)]
struct AddWordResponse {
    uuid: String,
}

async fn add_word(
    State(state): State<Arc<AppState>>,
    request: Result<Json<UserDictWordInput>, JsonRejection>,
) -> Result<impl IntoResponse, AdminError> {
    let Json(word) = request.map_err(|_| AdminError::bad_request("JSONリクエストが不正です"))?;
    validate_word(&word)?;
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    state
        .cache
        .clear()
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    let uuid = state
        .engine
        .add_user_dict_word(&word)
        .await
        .map_err(AdminError::engine)?;
    Ok((
        StatusCode::CREATED,
        Json(AddWordResponse {
            uuid: uuid.to_string(),
        }),
    ))
}

async fn update_word(
    State(state): State<Arc<AppState>>,
    Path(word_uuid): Path<String>,
    request: Result<Json<UserDictWordInput>, JsonRejection>,
) -> Result<StatusCode, AdminError> {
    let Json(word) = request.map_err(|_| AdminError::bad_request("JSONリクエストが不正です"))?;
    validate_word(&word)?;
    let uuid = parse_uuid(&word_uuid)?;
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    state
        .cache
        .clear()
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    state
        .engine
        .update_user_dict_word(uuid, &word)
        .await
        .map_err(AdminError::engine)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_word(
    State(state): State<Arc<AppState>>,
    Path(word_uuid): Path<String>,
) -> Result<StatusCode, AdminError> {
    let uuid = parse_uuid(&word_uuid)?;
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    state
        .cache
        .clear()
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    state
        .engine
        .delete_user_dict_word(uuid)
        .await
        .map_err(AdminError::engine)?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_word(word: &UserDictWordInput) -> Result<(), AdminError> {
    word.validate().map_err(AdminError::bad_request)
}

fn parse_uuid(value: &str) -> Result<Uuid, AdminError> {
    Uuid::parse_str(value).map_err(|_| AdminError::bad_request("単語UUIDが不正です"))
}

struct AdminError {
    status: StatusCode,
    message: &'static str,
    source: Option<Error>,
}

impl AdminError {
    fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
            source: None,
        }
    }

    fn engine(source: Error) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "TTS Engine の辞書操作に失敗しました",
            source: Some(source),
        }
    }

    fn internal(message: &'static str, source: Option<Error>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message,
            source,
        }
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

impl IntoResponse for AdminError {
    fn into_response(self) -> axum::response::Response {
        if let Some(source) = self.source {
            eprintln!("ユーザー辞書エラー: {source:#}");
        }
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}
