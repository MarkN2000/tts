use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::{
    admin::static_response, generate_cached_audio, get_audio, get_speakers, license_query,
    plain_text_url_response, AppError, AppState, TtsRequest,
};

const PAGE: &str = include_str!("../web/webui.html");
const STYLES: &str = include_str!("../web/webui.css");
const SCRIPT: &str = include_str!("../web/webui.js");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/webui", get(page))
        .route("/webui.css", get(styles))
        .route("/webui.js", get(script))
        .route("/api/webui/{engine}/speakers", get(get_speakers))
        .route(
            "/api/webui/{engine}/tts",
            get(generate_audio).post(generate_audio),
        )
        .route("/audio/{engine}/{filename}", get(get_audio))
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

async fn generate_audio(
    State(state): State<Arc<AppState>>,
    Path(engine_id): Path<String>,
    Query(request): Query<TtsRequest>,
) -> Result<Response<Body>, AppError> {
    if !state.engines.contains_key(&engine_id) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let generated = generate_cached_audio(&state, &engine_id, request).await?;
    let url = webui_audio_url(&engine_id, &generated.audio_id, &generated.license);
    Ok(plain_text_url_response(url))
}

fn webui_audio_url(engine_id: &str, audio_id: &str, license: &str) -> String {
    format!(
        "/audio/{engine_id}/{audio_id}.ogg?{}",
        license_query(license)
    )
}

#[cfg(test)]
mod tests {
    use super::webui_audio_url;

    #[test]
    fn web_ui音声urlは管理画面と同一オリジンの相対パスにする() {
        assert_eq!(
            webui_audio_url("aivisspeech", "audio-id", "CC0"),
            "/audio/aivisspeech/audio-id.ogg?license=CC0"
        );
    }
}
