use std::{path::Path as FilePath, sync::Arc};

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
    cache::{clear_all, usage_all, CacheUsage},
    config::Config,
    dictionary::{is_valid_pronunciation, UserDictWordInput},
    engine::UserDictPreviewError,
    updater::ApplyError,
    validate_runtime_config, AppState, EngineState,
};

const PAGE: &str = include_str!("../web/dictionary.html");
const STYLES: &str = include_str!("../web/dictionary.css");
const SCRIPT: &str = include_str!("../web/dictionary.js");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/settings", get(page))
        .route("/settings.css", get(styles))
        .route("/settings.js", get(script))
        .route("/api/settings", get(settings_info))
        .route("/api/cache", get(cache_info).delete(clear_cache))
        .route("/api/engines/{engine}/user-dict", get(load_dictionary))
        .route("/api/version", get(version_info))
        .route("/api/update", get(check_update).post(apply_update))
        .route("/api/restart", post(restart_server))
        .route(
            "/api/engines/{engine}/user-dict/preview",
            post(preview_word),
        )
        .route("/api/engines/{engine}/user-dict/words", post(add_word))
        .route(
            "/api/engines/{engine}/user-dict/words/{word_uuid}",
            axum::routing::put(update_word).delete(delete_word),
        )
        .with_state(state)
}

#[derive(Serialize)]
struct SettingsInfo {
    engines: Vec<EngineInfo>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct EngineInfo {
    id: String,
    name: String,
    public_tts_url: String,
    public_speakers_url: String,
}

async fn settings_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(build_settings_info(
        &state.config.public_base_url,
        &state.config.api_revision,
        &state.config.engines,
    ))
}

fn build_settings_info(
    public_base_url: &str,
    api_revision: &str,
    engines: &[crate::config::EngineConfig],
) -> SettingsInfo {
    let public_base_url = public_base_url.trim_end_matches('/');
    SettingsInfo {
        engines: engines
            .iter()
            .map(|engine| EngineInfo {
                id: engine.id.clone(),
                name: engine.name.clone(),
                public_tts_url: format!("{public_base_url}/api/{}/{}/tts", api_revision, engine.id),
                public_speakers_url: format!(
                    "{public_base_url}/api/{}/{}/speakers",
                    api_revision, engine.id
                ),
            })
            .collect(),
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct CacheInfo {
    used_bytes: u64,
    max_bytes: u64,
    file_count: u64,
    cache_days: u64,
}

async fn cache_info(State(state): State<Arc<AppState>>) -> Result<Json<CacheInfo>, AdminError> {
    let caches = state
        .engines
        .values()
        .map(|engine| &engine.cache)
        .collect::<Vec<_>>();
    let usage = usage_all(&caches)
        .await
        .map_err(|error| AdminError::internal("キャッシュ情報を取得できません", Some(error)))?;
    Ok(Json(build_cache_info(
        usage,
        state.config.cache_days,
        state.config.cache_max_mb,
    )))
}

fn build_cache_info(usage: CacheUsage, cache_days: u64, cache_max_mb: u64) -> CacheInfo {
    CacheInfo {
        used_bytes: usage.used_bytes,
        max_bytes: cache_max_mb * 1024 * 1024,
        file_count: usage.file_count,
        cache_days,
    }
}

async fn clear_cache(State(state): State<Arc<AppState>>) -> Result<StatusCode, AdminError> {
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    let caches = state
        .engines
        .values()
        .map(|engine| &engine.cache)
        .collect::<Vec<_>>();
    clear_all(&caches)
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn version_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(ServerVersionInfo {
        update: state.updater.version_info(),
        restart_supported: restart_supported(),
        instance_id: state.instance_id,
    })
}

#[derive(Serialize)]
struct ServerVersionInfo {
    #[serde(flatten)]
    update: crate::updater::VersionInfo,
    restart_supported: bool,
    instance_id: Uuid,
}

const fn restart_supported() -> bool {
    cfg!(target_os = "linux")
}

async fn check_update(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, UpdateApiError> {
    state
        .updater
        .check()
        .await
        .map(Json)
        .map_err(UpdateApiError::failed)
}

async fn apply_update(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, UpdateApiError> {
    let _maintenance = state
        .maintenance_lock
        .try_lock()
        .map_err(|_| UpdateApiError::conflict("アップデートまたは再起動が進行中です"))?;
    let updated = state.updater.apply().await.map_err(|error| match error {
        ApplyError::NoUpdate => UpdateApiError::conflict("利用できるアップデートはありません"),
        ApplyError::Restarting => UpdateApiError::conflict("更新を適用して再起動中です"),
        ApplyError::Failed(error) => UpdateApiError::failed(error),
    })?;
    let _ = state.shutdown_sender.send(true);
    Ok((StatusCode::ACCEPTED, Json(updated)))
}

async fn restart_server(State(state): State<Arc<AppState>>) -> Result<StatusCode, RestartApiError> {
    if !restart_supported() {
        return Err(RestartApiError::unsupported());
    }
    let _maintenance = state
        .maintenance_lock
        .try_lock()
        .map_err(|_| RestartApiError::conflict())?;
    validate_and_request_restart(&state.config_path, &state.shutdown_sender).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn validate_and_request_restart(
    config_path: &FilePath,
    shutdown_sender: &tokio::sync::watch::Sender<bool>,
) -> Result<(), RestartApiError> {
    let config = Config::load(config_path).map_err(RestartApiError::invalid_config)?;
    validate_runtime_config(&config)
        .await
        .map_err(RestartApiError::invalid_config)?;
    let _ = shutdown_sender.send(true);
    Ok(())
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

pub(crate) fn static_response(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-store")],
        body,
    )
}

async fn load_dictionary(
    State(state): State<Arc<AppState>>,
    Path(engine_id): Path<String>,
) -> Result<impl IntoResponse, AdminError> {
    let engine = find_engine(&state, &engine_id)?;
    let dictionary = engine
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
    Path(engine_id): Path<String>,
    request: Result<Json<PreviewRequest>, JsonRejection>,
) -> Result<impl IntoResponse, AdminError> {
    let engine = find_engine(&state, &engine_id)?;
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
    let wav = match engine
        .engine
        .synthesize_user_dict_preview(
            &engine.config.default_id,
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
    Path(engine_id): Path<String>,
    request: Result<Json<UserDictWordInput>, JsonRejection>,
) -> Result<impl IntoResponse, AdminError> {
    let engine = find_engine(&state, &engine_id)?;
    let Json(word) = request.map_err(|_| AdminError::bad_request("JSONリクエストが不正です"))?;
    validate_word(&word)?;
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    engine
        .cache
        .clear()
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    let uuid = engine
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
    Path((engine_id, word_uuid)): Path<(String, String)>,
    request: Result<Json<UserDictWordInput>, JsonRejection>,
) -> Result<StatusCode, AdminError> {
    let engine = find_engine(&state, &engine_id)?;
    let Json(word) = request.map_err(|_| AdminError::bad_request("JSONリクエストが不正です"))?;
    validate_word(&word)?;
    let uuid = parse_uuid(&word_uuid)?;
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    engine
        .cache
        .clear()
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    engine
        .engine
        .update_user_dict_word(uuid, &word)
        .await
        .map_err(AdminError::engine)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_word(
    State(state): State<Arc<AppState>>,
    Path((engine_id, word_uuid)): Path<(String, String)>,
) -> Result<StatusCode, AdminError> {
    let engine = find_engine(&state, &engine_id)?;
    let uuid = parse_uuid(&word_uuid)?;
    let _permit = state
        .generation_lock
        .acquire()
        .await
        .map_err(|_| AdminError::internal("排他制御を開始できません", None))?;
    engine
        .cache
        .clear()
        .await
        .map_err(|error| AdminError::internal("音声キャッシュを削除できません", Some(error)))?;
    engine
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

fn find_engine<'a>(state: &'a AppState, engine_id: &str) -> Result<&'a EngineState, AdminError> {
    state
        .engines
        .get(engine_id)
        .ok_or_else(|| AdminError::not_found("指定されたTTS Engineがありません"))
}

struct AdminError {
    status: StatusCode,
    message: &'static str,
    source: Option<Error>,
}

impl AdminError {
    fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
            source: None,
        }
    }
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
            eprintln!("管理APIエラー: {source:#}");
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

struct UpdateApiError {
    status: StatusCode,
    message: String,
}

#[derive(Debug)]
struct RestartApiError {
    status: StatusCode,
    message: String,
}

impl RestartApiError {
    fn unsupported() -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "設定画面からの再起動はLinuxだけに対応しています".to_owned(),
        }
    }

    fn conflict() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: "アップデートまたは再起動が進行中です".to_owned(),
        }
    }

    fn invalid_config(source: Error) -> Self {
        eprintln!("再起動前の設定検証エラー: {source:#}");
        Self {
            status: StatusCode::BAD_REQUEST,
            message: format!("設定を反映できません: {source:#}"),
        }
    }
}

impl UpdateApiError {
    fn conflict(message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.to_owned(),
        }
    }

    fn failed(source: Error) -> Self {
        eprintln!("アップデートエラー: {source:#}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("アップデート処理に失敗しました: {source:#}"),
        }
    }
}

#[derive(Serialize)]
struct UpdateErrorResponse {
    error: String,
}

impl IntoResponse for UpdateApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(UpdateErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

impl IntoResponse for RestartApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(UpdateErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use crate::cache::CacheUsage;
    use crate::config::EngineConfig;
    use crate::updater::UpdateManager;
    use uuid::Uuid;

    use super::{
        build_cache_info, build_settings_info, restart_supported, validate_and_request_restart,
        ServerVersionInfo,
    };

    #[test]
    fn 設定画面のリンクを実行中の設定から組み立てる() {
        let settings = build_settings_info(
            "https://tts.example.com/",
            "v2-test",
            &[EngineConfig {
                id: "aivisspeech".to_owned(),
                name: "AivisSpeech".to_owned(),
                engine_url: "http://127.0.0.1:10101".to_owned(),
                default_id: "1".to_owned(),
                attribution: crate::config::AttributionConfig::LicenseFromPolicy,
            }],
        );

        assert_eq!(
            settings.engines[0].public_tts_url,
            "https://tts.example.com/api/v2-test/aivisspeech/tts"
        );
        assert_eq!(
            settings.engines[0].public_speakers_url,
            "https://tts.example.com/api/v2-test/aivisspeech/speakers"
        );
    }

    #[test]
    fn キャッシュ情報に使用量と設定値を含める() {
        let info = build_cache_info(
            CacheUsage {
                used_bytes: 300,
                file_count: 2,
            },
            31,
            1024,
        );

        assert_eq!(info.used_bytes, 300);
        assert_eq!(info.max_bytes, 1024 * 1024 * 1024);
        assert_eq!(info.file_count, 2);
        assert_eq!(info.cache_days, 31);
    }

    #[test]
    fn バージョン情報に再起動対応と起動idを含める() {
        let instance_id = Uuid::new_v4();
        let info = ServerVersionInfo {
            update: UpdateManager::new(env::current_exe().unwrap())
                .unwrap()
                .version_info(),
            restart_supported: restart_supported(),
            instance_id,
        };

        let json = serde_json::to_value(info).unwrap();

        assert_eq!(json["restart_supported"], cfg!(target_os = "linux"));
        assert_eq!(json["instance_id"], instance_id.to_string());
        assert!(json["current_version"].is_string());
        assert!(json["supported"].is_boolean());
    }

    #[tokio::test]
    async fn 不正な設定では終了通知を送らない() {
        let missing_config =
            env::temp_dir().join(format!("tts-server-missing-config-{}.toml", Uuid::new_v4()));
        let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);

        let error = validate_and_request_restart(&missing_config, &shutdown_sender)
            .await
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(!*shutdown_receiver.borrow());
    }
}
