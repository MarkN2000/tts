use std::sync::Arc;

use axum::{
    extract::State,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use crate::{
    admin::static_response, generate_cached_audio, get_audio, get_speakers, AppError, AppState,
    TtsRequest,
};

const PAGE: &str = include_str!("../web/webui.html");
const STYLES: &str = include_str!("../web/webui.css");
const SCRIPT: &str = include_str!("../web/webui.js");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/webui", get(page))
        .route("/webui.css", get(styles))
        .route("/webui.js", get(script))
        .route("/speakers", get(get_speakers))
        .route("/api/webui/tts", post(generate_audio))
        .route("/audio/{filename}", get(get_audio))
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

#[derive(Serialize)]
struct WebuiTtsResponse {
    license: String,
    url: String,
}

async fn generate_audio(
    State(state): State<Arc<AppState>>,
    Json(request): Json<TtsRequest>,
) -> Result<Json<WebuiTtsResponse>, AppError> {
    let generated = generate_cached_audio(&state, request).await?;
    Ok(Json(WebuiTtsResponse {
        license: generated.license,
        url: webui_audio_url(&generated.audio_id),
    }))
}

fn webui_audio_url(audio_id: &str) -> String {
    format!("/audio/{audio_id}.ogg")
}

#[cfg(test)]
mod tests {
    use super::webui_audio_url;

    #[test]
    fn web_ui音声urlは管理画面と同一オリジンの相対パスにする() {
        assert_eq!(webui_audio_url("audio-id"), "/audio/audio-id.ogg");
    }
}
