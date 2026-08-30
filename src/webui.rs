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
    admin::static_response, attribution_query, generate_cached_audio, get_audio, get_speakers,
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
    let url = webui_audio_url(&engine_id, &generated.audio_id, &generated.attribution);
    Ok(plain_text_url_response(url))
}

fn webui_audio_url(
    engine_id: &str,
    audio_id: &str,
    attribution: &crate::engine::SpeakerAttribution,
) -> String {
    format!(
        "/audio/{engine_id}/{audio_id}.ogg?{}",
        attribution_query(attribution)
    )
}

#[cfg(test)]
mod tests {
    use crate::engine::SpeakerAttribution;

    use super::webui_audio_url;

    #[test]
    fn web_ui音声urlは管理画面と同一オリジンの相対パスにする() {
        assert_eq!(
            webui_audio_url(
                "aivisspeech",
                "audio-id",
                &SpeakerAttribution::License("CC0".to_owned())
            ),
            "/audio/aivisspeech/audio-id.ogg?license=CC0"
        );
    }

    #[test]
    fn web_ui音声urlへクレジットを含める() {
        assert_eq!(
            webui_audio_url(
                "voicevox",
                "audio-id",
                &SpeakerAttribution::Credit("VOICEVOX:ずんだもん".to_owned())
            ),
            "/audio/voicevox/audio-id.ogg?credit=VOICEVOX%3A%E3%81%9A%E3%82%93%E3%81%A0%E3%82%82%E3%82%93"
        );
    }
}
