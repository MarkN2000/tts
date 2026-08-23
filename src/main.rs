mod admin;
mod cache;
mod config;
mod converter;
mod dictionary;
mod engine;
mod webui;

use std::{collections::HashMap, env, fmt, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{
        header::{CONTENT_TYPE, RETRY_AFTER},
        Response, StatusCode,
    },
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    net::TcpListener,
    sync::{watch, Semaphore, SemaphorePermit},
    time,
};

use crate::{
    cache::{AudioCache, CacheSignature},
    config::Config,
    converter::FfmpegConverter,
    engine::{EngineClient, SpeakerMeta},
};

const MAX_GENERATION_REQUESTS: usize = 10;
const GENERATION_RETRY_AFTER_SECONDS: &str = "10";

pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) engine: EngineClient,
    speakers: HashMap<String, SpeakerMeta>,
    speakers_json: Bytes,
    pub(crate) cache: AudioCache,
    converter: FfmpegConverter,
    pub(crate) generation_lock: Semaphore,
    generation_slots: Semaphore,
}

#[derive(Deserialize)]
pub(crate) struct TtsRequest {
    id: Option<String>,
    text: String,
}

#[derive(Serialize)]
struct TtsResponse {
    license: String,
    url: String,
}

pub(crate) struct GeneratedAudio {
    pub(crate) audio_id: String,
    pub(crate) license: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let executable = env::current_exe().context("実行ファイルの場所を取得できません")?;
    let executable_directory = executable
        .parent()
        .context("実行ファイルのディレクトリを取得できません")?;
    let config = Config::load(&executable_directory.join("config.toml"))?;
    let converter = FfmpegConverter::new(
        config.ffmpeg_path.clone(),
        config.codec,
        config.bitrate_kbps,
    );
    converter.verify().await?;

    let engine = EngineClient::new(config.engine_url.clone());
    let speaker_catalog = engine.load_speakers().await?;
    if !speaker_catalog.speakers.contains_key(&config.default_id) {
        anyhow::bail!(
            "default_id {} は Engine の話者一覧に存在しません",
            config.default_id
        );
    }

    let cache = AudioCache::new(
        config.cache_dir.clone(),
        config.cache_days,
        config.cache_max_mb,
        CacheSignature {
            engine_url: config.engine_url.clone(),
            cache_revision: config.cache_revision,
            codec: config.codec,
            bitrate_kbps: config.bitrate_kbps,
        },
    );
    cache.prepare().await?;

    let api_path = format!("/api/{}/tts", config.api_revision);
    let listen = config.listen.clone();
    let admin_address = config.admin_address()?;
    let state = Arc::new(AppState {
        config,
        engine,
        speakers: speaker_catalog.speakers,
        speakers_json: Bytes::from(speaker_catalog.raw_json),
        cache,
        converter,
        generation_lock: Semaphore::new(1),
        generation_slots: Semaphore::new(MAX_GENERATION_REQUESTS),
    });

    spawn_cache_cleanup(Arc::clone(&state));

    let public_app = Router::new()
        .route(&api_path, post(generate_audio))
        .route("/speakers", get(get_speakers))
        .route("/audio/{filename}", get(get_audio))
        .layer(DefaultBodyLimit::disable())
        .with_state(Arc::clone(&state));
    let admin_app = admin::router(Arc::clone(&state)).merge(webui::router(state));
    let public_listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("{listen} で待受を開始できません"))?;
    let admin_listener = TcpListener::bind(admin_address)
        .await
        .with_context(|| format!("{admin_address} で管理画面の待受を開始できません"))?;

    println!("TTS サーバーを http://{listen}{api_path} で開始しました");
    println!("音声生成 Web UI を http://{admin_address}/webui で開始しました");
    println!("辞書管理画面を http://{admin_address}/dictionary で開始しました");

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_sender.send(true);
    });

    tokio::try_join!(
        axum::serve(public_listener, public_app)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver.clone())),
        axum::serve(admin_listener, admin_app)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver)),
    )
    .map(|_| ())
    .context("HTTP サーバーが異常終了しました")
}

async fn generate_audio(
    State(state): State<Arc<AppState>>,
    Json(request): Json<TtsRequest>,
) -> Result<Json<TtsResponse>, AppError> {
    let generated = generate_cached_audio(&state, request).await?;
    Ok(Json(TtsResponse {
        license: generated.license,
        url: public_audio_url(&state.config.public_base_url, &generated.audio_id),
    }))
}

fn public_audio_url(public_base_url: &str, audio_id: &str) -> String {
    format!("{public_base_url}/audio/{audio_id}.ogg")
}

pub(crate) async fn generate_cached_audio(
    state: &AppState,
    request: TtsRequest,
) -> Result<GeneratedAudio> {
    let speaker_id = request
        .id
        .as_deref()
        .filter(|id| state.speakers.contains_key(*id))
        .unwrap_or(&state.config.default_id)
        .to_owned();
    let audio_id = make_audio_id(&speaker_id, &request.text);

    if state.cache.find(&audio_id).await?.is_none() {
        let _request_slot = try_enter_generation_queue(&state.generation_slots)?;
        let _permit = state
            .generation_lock
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("音声生成ロックが閉じられました"))?;

        state.cache.ensure_ready().await?;
        if state.cache.find(&audio_id).await?.is_none() {
            let wav = state.engine.synthesize(&speaker_id, &request.text).await?;
            state
                .converter
                .convert(&wav, &state.cache, &audio_id)
                .await?;
            state.cache.cleanup_after_generation(&audio_id).await?;
        }
    }

    let speaker = &state.speakers[&speaker_id];
    Ok(GeneratedAudio {
        audio_id,
        license: speaker.license.clone(),
    })
}

pub(crate) async fn get_speakers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/json")],
        state.speakers_json.clone(),
    )
}

pub(crate) async fn get_audio(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
) -> Result<Response<Body>, StatusCode> {
    let Some(audio_id) = filename.strip_suffix(".ogg") else {
        return Err(StatusCode::NOT_FOUND);
    };
    if !is_valid_audio_id(audio_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    let path = state
        .cache
        .find(audio_id)
        .await
        .map_err(|error| {
            eprintln!("キャッシュ確認エラー: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let bytes = fs::read(path).await.map_err(|error| {
        eprintln!("音声ファイル読込エラー: {error:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "audio/ogg")
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn make_audio_id(speaker_id: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((speaker_id.len() as u64).to_be_bytes());
    hasher.update(speaker_id.as_bytes());
    hasher.update(text.as_bytes());
    let hash = hasher.finalize();
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_valid_audio_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn spawn_cache_cleanup(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            let Ok(_permit) = state.generation_lock.acquire().await else {
                return;
            };
            if let Err(error) = state.cache.cleanup().await {
                eprintln!("期限切れキャッシュの削除に失敗しました: {error:#}");
            }
        }
    });
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("終了シグナルを待機できません: {error}");
    }
}

async fn wait_for_shutdown(mut receiver: watch::Receiver<bool>) {
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
}

pub(crate) struct AppError(anyhow::Error);

#[derive(Debug)]
struct GenerationQueueFull;

fn try_enter_generation_queue(
    slots: &Semaphore,
) -> std::result::Result<SemaphorePermit<'_>, GenerationQueueFull> {
    slots.try_acquire().map_err(|_| GenerationQueueFull)
}

impl fmt::Display for GenerationQueueFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("音声生成の受付上限に達しました")
    }
}

impl std::error::Error for GenerationQueueFull {}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        if self.0.is::<GenerationQueueFull>() {
            eprintln!("音声生成の受付上限に達したためリクエストを拒否しました");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(RETRY_AFTER, GENERATION_RETRY_AFTER_SECONDS)],
                "音声生成が混み合っています",
            )
                .into_response();
        }
        eprintln!("音声生成エラー: {:#}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, "音声生成に失敗しました").into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        http::{header::RETRY_AFTER, StatusCode},
        response::IntoResponse,
    };
    use tokio::sync::Semaphore;

    use super::{
        is_valid_audio_id, make_audio_id, public_audio_url, try_enter_generation_queue, AppError,
        GenerationQueueFull, GENERATION_RETRY_AFTER_SECONDS, MAX_GENERATION_REQUESTS,
    };

    #[test]
    fn 同じ話者とテキストは同じaudio_idになる() {
        assert_eq!(
            make_audio_id("1", "こんにちは"),
            make_audio_id("1", "こんにちは")
        );
        assert_ne!(
            make_audio_id("1", "こんにちは"),
            make_audio_id("2", "こんにちは")
        );
        assert_ne!(
            make_audio_id("1", "こんにちは"),
            make_audio_id("1", "こんばんは")
        );
    }

    #[test]
    fn audio_idは64文字の16進数だけを許可する() {
        let id = make_audio_id("1", "text");
        assert!(is_valid_audio_id(&id));
        assert!(!is_valid_audio_id("../config.toml"));
        assert!(!is_valid_audio_id(&"g".repeat(64)));
    }

    #[test]
    fn 未生成音声は同時に10件まで受け付ける() {
        let slots = Semaphore::new(MAX_GENERATION_REQUESTS);
        let mut permits = (0..MAX_GENERATION_REQUESTS)
            .map(|_| try_enter_generation_queue(&slots).expect("10件までは取得できる"))
            .collect::<Vec<_>>();

        assert!(try_enter_generation_queue(&slots).is_err());
        permits.pop();
        assert!(try_enter_generation_queue(&slots).is_ok());
    }

    #[test]
    fn 受付上限超過は再試行時間付きの503を返す() {
        let response = AppError(anyhow::Error::new(GenerationQueueFull)).into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers()[RETRY_AFTER],
            GENERATION_RETRY_AFTER_SECONDS
        );
    }

    #[test]
    fn 公開音声urlは公開用base_urlを使用する() {
        assert_eq!(
            public_audio_url("https://tts.example.com", "audio-id"),
            "https://tts.example.com/audio/audio-id.ogg"
        );
    }
}
