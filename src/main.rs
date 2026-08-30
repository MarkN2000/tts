mod admin;
mod atomic_file;
mod cache;
mod config;
mod converter;
mod dictionary;
mod engine;
mod updater;
mod webui;

use std::{collections::HashMap, env, fmt, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{
        header::{HeaderName, HeaderValue, CONTENT_TYPE, RETRY_AFTER},
        Response, StatusCode,
    },
    middleware::{self, Next},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    net::TcpListener,
    sync::{watch, Mutex, Semaphore, SemaphorePermit},
    time,
};
use tower::Layer;
use tower_http::normalize_path::{NormalizePath, NormalizePathLayer};
use uuid::Uuid;

use crate::{
    cache::{cleanup_all, prepare_cache_root, AudioCache, CacheSignature},
    config::{Config, EngineConfig},
    converter::FfmpegConverter,
    engine::{EngineClient, SpeakerAttribution},
    updater::UpdateManager,
};

const MAX_GENERATION_REQUESTS: usize = 10;
const GENERATION_RETRY_AFTER_SECONDS: &str = "10";
const LEGACY_TTS_PATH: &str = "/api/v1/tts";
const LEGACY_SPEAKERS_PATH: &str = "/api/v1/speakers";

pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) config_path: PathBuf,
    pub(crate) engines: HashMap<String, EngineState>,
    converter: FfmpegConverter,
    pub(crate) generation_lock: Semaphore,
    generation_slots: Semaphore,
    pub(crate) updater: UpdateManager,
    pub(crate) shutdown_sender: watch::Sender<bool>,
    pub(crate) maintenance_lock: Mutex<()>,
    pub(crate) instance_id: Uuid,
}

pub(crate) struct EngineState {
    pub(crate) config: EngineConfig,
    pub(crate) engine: EngineClient,
    speakers: HashMap<String, SpeakerAttribution>,
    speakers_json: Bytes,
    pub(crate) cache: AudioCache,
}

#[derive(Deserialize)]
pub(crate) struct TtsRequest {
    speaker: Option<String>,
    text: String,
}

pub(crate) struct GeneratedAudio {
    pub(crate) audio_id: String,
    pub(crate) attribution: SpeakerAttribution,
}

#[tokio::main]
async fn main() -> Result<()> {
    if env::args().nth(1).as_deref() == Some("--version") {
        println!("tts-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let executable = env::current_exe().context("実行ファイルの場所を取得できません")?;
    let executable_directory = executable
        .parent()
        .context("実行ファイルのディレクトリを取得できません")?;
    let config_path = executable_directory.join("config.toml");
    let config = Config::load(&config_path)?;
    let (converter, engines) = build_runtime(&config).await?;
    prepare_runtime_cache(&config, &engines).await?;

    let api_path = format!("/api/{}/{{engine}}/tts", config.api_revision);
    let speakers_path = format!("/api/{}/{{engine}}/speakers", config.api_revision);
    let listen = config.public_address()?;
    let admin_address = config.admin_address()?;
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let state = Arc::new(AppState {
        config,
        config_path,
        engines,
        converter,
        generation_lock: Semaphore::new(1),
        generation_slots: Semaphore::new(MAX_GENERATION_REQUESTS),
        updater: UpdateManager::new(executable)?,
        shutdown_sender: shutdown_sender.clone(),
        maintenance_lock: Mutex::new(()),
        instance_id: Uuid::new_v4(),
    });

    spawn_cache_cleanup(Arc::clone(&state));

    let public_app = Router::new()
        .route(&api_path, get(generate_audio).post(generate_audio))
        .route(
            LEGACY_TTS_PATH,
            get(generate_legacy_audio).post(generate_legacy_audio),
        )
        .route(LEGACY_SPEAKERS_PATH, get(get_legacy_speakers))
        .route(&speakers_path, get(get_speakers))
        .route("/audio/{engine}/{filename}", get(get_audio))
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn(add_noindex_header))
        .with_state(Arc::clone(&state));
    let public_app = normalize_public_paths(public_app);
    let admin_app = admin::router(Arc::clone(&state))
        .merge(webui::router(state))
        .layer(middleware::from_fn(add_noindex_header));
    let public_listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("{listen} で待受を開始できません"))?;
    let admin_listener = TcpListener::bind(admin_address)
        .await
        .with_context(|| format!("{admin_address} で管理画面の待受を開始できません"))?;

    println!("TTS サーバーを http://{listen}{api_path} で開始しました");
    println!("音声生成 Web UI を http://{admin_address}/webui で開始しました");
    println!("設定画面を http://{admin_address}/settings で開始しました");

    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_sender.send(true);
    });

    tokio::try_join!(
        axum::serve(
            public_listener,
            axum::ServiceExt::<Request>::into_make_service(public_app),
        )
        .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver.clone())),
        axum::serve(admin_listener, admin_app)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver)),
    )
    .map(|_| ())
    .context("HTTP サーバーが異常終了しました")
}

fn normalize_public_paths(router: Router) -> NormalizePath<Router> {
    NormalizePathLayer::trim_trailing_slash().layer(router)
}

async fn build_runtime(config: &Config) -> Result<(FfmpegConverter, HashMap<String, EngineState>)> {
    let converter = FfmpegConverter::new(
        config.ffmpeg_path.clone(),
        config.codec,
        config.bitrate_kbps,
    );
    converter.verify().await?;

    let mut engines = HashMap::new();
    for engine_config in &config.engines {
        let engine = EngineClient::new(engine_config.engine_url.clone());
        let speaker_catalog = engine
            .load_speakers(engine_config.attribution.credit_template())
            .await
            .with_context(|| format!("Engine {} の話者一覧を取得できません", engine_config.id))?;
        if !speaker_catalog
            .speakers
            .contains_key(&engine_config.default_id)
        {
            anyhow::bail!(
                "Engine {} の default_id {} は話者一覧に存在しません",
                engine_config.id,
                engine_config.default_id
            );
        }
        let cache = AudioCache::new(
            config.cache_dir.join(&engine_config.id),
            config.cache_days,
            CacheSignature {
                engine_url: engine_config.engine_url.clone(),
                cache_revision: config.cache_revision,
                codec: config.codec,
                bitrate_kbps: config.bitrate_kbps,
            },
        );
        engines.insert(
            engine_config.id.clone(),
            EngineState {
                config: engine_config.clone(),
                engine,
                speakers: speaker_catalog.speakers,
                speakers_json: Bytes::from(speaker_catalog.raw_json),
                cache,
            },
        );
    }
    Ok((converter, engines))
}

pub(crate) async fn validate_runtime_config(config: &Config) -> Result<()> {
    build_runtime(config).await.map(|_| ())
}

async fn prepare_runtime_cache(
    config: &Config,
    engines: &HashMap<String, EngineState>,
) -> Result<()> {
    let engine_ids = config
        .engines
        .iter()
        .map(|engine| engine.id.clone())
        .collect::<Vec<_>>();
    prepare_cache_root(&config.cache_dir, &engine_ids).await?;
    for engine in engines.values() {
        engine.cache.prepare().await?;
    }
    let caches = engines
        .values()
        .map(|engine| &engine.cache)
        .collect::<Vec<_>>();
    cleanup_all(&caches, config.cache_max_mb * 1024 * 1024, None).await?;
    Ok(())
}

async fn generate_audio(
    State(state): State<Arc<AppState>>,
    Path(engine_id): Path<String>,
    Query(request): Query<TtsRequest>,
) -> Result<Response<Body>, AppError> {
    if !state.engines.contains_key(&engine_id) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    generate_audio_response(state.as_ref(), &engine_id, request).await
}

async fn generate_legacy_audio(
    State(state): State<Arc<AppState>>,
    Query(request): Query<TtsRequest>,
) -> Result<Response<Body>, AppError> {
    let engine_id = legacy_engine_id(&state.config.engines);
    generate_audio_response(state.as_ref(), engine_id, request).await
}

fn legacy_engine_id(engines: &[EngineConfig]) -> &str {
    &engines
        .first()
        .expect("設定検証によりEngineは1件以上あります")
        .id
}

async fn generate_audio_response(
    state: &AppState,
    engine_id: &str,
    request: TtsRequest,
) -> Result<Response<Body>, AppError> {
    let generated = generate_cached_audio(state, engine_id, request).await?;
    let url = public_audio_url(
        &state.config.public_base_url,
        engine_id,
        &generated.audio_id,
        &generated.attribution,
    );
    Ok(plain_text_url_response(url))
}

fn public_audio_url(
    public_base_url: &str,
    engine_id: &str,
    audio_id: &str,
    attribution: &SpeakerAttribution,
) -> String {
    format!(
        "{public_base_url}/audio/{engine_id}/{audio_id}.ogg?{}",
        attribution_query(attribution)
    )
}

pub(crate) fn attribution_query(attribution: &SpeakerAttribution) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").expect("固定URLは常に有効");
    let (key, value) = match attribution {
        SpeakerAttribution::License(license) => ("license", license),
        SpeakerAttribution::Credit(credit) => ("credit", credit),
    };
    url.query_pairs_mut().append_pair(key, value);
    url.query().expect("利用情報のクエリが存在する").to_owned()
}

pub(crate) fn plain_text_url_response(url: String) -> Response<Body> {
    ([(CONTENT_TYPE, "text/plain; charset=utf-8")], url).into_response()
}

async fn add_noindex_header(request: Request, next: Next) -> Response<Body> {
    with_noindex_header(next.run(request).await)
}

fn with_noindex_header(mut response: Response<Body>) -> Response<Body> {
    response.headers_mut().insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex"),
    );
    response
}

pub(crate) async fn generate_cached_audio(
    state: &AppState,
    engine_id: &str,
    request: TtsRequest,
) -> Result<GeneratedAudio> {
    let engine = state
        .engines
        .get(engine_id)
        .with_context(|| format!("未知のEngine IDです: {engine_id}"))?;
    let speaker_id = resolve_speaker_id(
        &engine.speakers,
        &engine.config.default_id,
        request.speaker.as_deref(),
    )
    .to_owned();
    let audio_id = make_audio_id(engine_id, &speaker_id, &request.text);

    if engine.cache.find(&audio_id).await?.is_none() {
        let _request_slot = try_enter_generation_queue(&state.generation_slots)?;
        let _permit = state
            .generation_lock
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("音声生成ロックが閉じられました"))?;

        engine.cache.ensure_ready().await?;
        if engine.cache.find(&audio_id).await?.is_none() {
            let wav = engine.engine.synthesize(&speaker_id, &request.text).await?;
            state
                .converter
                .convert(&wav, &engine.cache, &audio_id)
                .await?;
            let preserved_path = engine.cache.audio_path(&audio_id);
            let caches = state
                .engines
                .values()
                .map(|engine| &engine.cache)
                .collect::<Vec<_>>();
            cleanup_all(
                &caches,
                state.config.cache_max_mb * 1024 * 1024,
                Some(&preserved_path),
            )
            .await?;
        }
    }

    let speaker = &engine.speakers[&speaker_id];
    Ok(GeneratedAudio {
        audio_id,
        attribution: speaker.clone(),
    })
}

fn resolve_speaker_id<'a, T>(
    speakers: &'a HashMap<String, T>,
    default_id: &'a str,
    requested_id: Option<&str>,
) -> &'a str {
    requested_id
        .and_then(|id| speakers.get_key_value(id).map(|(key, _)| key.as_str()))
        .unwrap_or(default_id)
}

pub(crate) async fn get_speakers(
    State(state): State<Arc<AppState>>,
    Path(engine_id): Path<String>,
) -> Response<Body> {
    speakers_response(state.as_ref(), &engine_id)
}

async fn get_legacy_speakers(State(state): State<Arc<AppState>>) -> Response<Body> {
    let engine_id = legacy_engine_id(&state.config.engines);
    speakers_response(state.as_ref(), engine_id)
}

fn speakers_response(state: &AppState, engine_id: &str) -> Response<Body> {
    let Some(engine) = state.engines.get(engine_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [(CONTENT_TYPE, "application/json")],
        engine.speakers_json.clone(),
    )
        .into_response()
}

pub(crate) async fn get_audio(
    State(state): State<Arc<AppState>>,
    Path((engine_id, filename)): Path<(String, String)>,
) -> Result<Response<Body>, StatusCode> {
    let engine = state.engines.get(&engine_id).ok_or(StatusCode::NOT_FOUND)?;
    let Some(audio_id) = filename.strip_suffix(".ogg") else {
        return Err(StatusCode::NOT_FOUND);
    };
    if !is_valid_audio_id(audio_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    let path = engine
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

fn make_audio_id(engine_id: &str, speaker_id: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((engine_id.len() as u64).to_be_bytes());
    hasher.update(engine_id.as_bytes());
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
            let caches = state
                .engines
                .values()
                .map(|engine| &engine.cache)
                .collect::<Vec<_>>();
            if let Err(error) =
                cleanup_all(&caches, state.config.cache_max_mb * 1024 * 1024, None).await
            {
                eprintln!("期限切れキャッシュの削除に失敗しました: {error:#}");
            }
        }
    });
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("終了シグナルを待機できません: {error}");
    }
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("SIGTERMを待機できません: {error}");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                eprintln!("終了シグナルを待機できません: {error}");
            }
        }
        _ = terminate.recv() => {}
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
    use std::collections::HashMap;

    use axum::{
        body::{to_bytes, Body},
        extract::Query,
        http::{
            header::{CONTENT_TYPE, RETRY_AFTER},
            Method, Request, Response, StatusCode, Uri,
        },
        response::IntoResponse,
        routing::get,
        Router,
    };
    use tokio::sync::Semaphore;
    use tower::ServiceExt;

    use super::{
        attribution_query, is_valid_audio_id, legacy_engine_id, make_audio_id,
        normalize_public_paths, plain_text_url_response, public_audio_url, resolve_speaker_id,
        try_enter_generation_queue, with_noindex_header, AppError, EngineConfig,
        GenerationQueueFull, SpeakerAttribution, TtsRequest, GENERATION_RETRY_AFTER_SECONDS,
        LEGACY_SPEAKERS_PATH, LEGACY_TTS_PATH, MAX_GENERATION_REQUESTS,
    };

    #[test]
    fn 同じ話者とテキストは同じaudio_idになる() {
        assert_eq!(
            make_audio_id("aivisspeech", "1", "こんにちは"),
            make_audio_id("aivisspeech", "1", "こんにちは")
        );
        assert_ne!(
            make_audio_id("aivisspeech", "1", "こんにちは"),
            make_audio_id("aivisspeech", "2", "こんにちは")
        );
        assert_ne!(
            make_audio_id("aivisspeech", "1", "こんにちは"),
            make_audio_id("aivisspeech", "1", "こんばんは")
        );
        assert_ne!(
            make_audio_id("aivisspeech", "1", "こんにちは"),
            make_audio_id("voicevox", "1", "こんにちは")
        );
    }

    #[test]
    fn v1互換urlは設定順先頭のengineを使用する() {
        let engines = [
            EngineConfig {
                id: "aivisspeech".to_owned(),
                name: "AivisSpeech".to_owned(),
                engine_url: "http://127.0.0.1:10101".to_owned(),
                default_id: "1".to_owned(),
                attribution: crate::config::AttributionConfig::LicenseFromPolicy,
            },
            EngineConfig {
                id: "voicevox".to_owned(),
                name: "VOICEVOX".to_owned(),
                engine_url: "http://127.0.0.1:50021".to_owned(),
                default_id: "3".to_owned(),
                attribution: crate::config::AttributionConfig::Credit {
                    template: "VOICEVOX:{speaker_name}".to_owned(),
                },
            },
        ];

        assert_eq!(LEGACY_TTS_PATH, "/api/v1/tts");
        assert_eq!(LEGACY_SPEAKERS_PATH, "/api/v1/speakers");
        assert_eq!(legacy_engine_id(&engines), "aivisspeech");
    }

    #[tokio::test]
    async fn 公開apiは末尾スラッシュを除去してルーティングする() {
        let app = normalize_public_paths(
            Router::new()
                .route(
                    "/api/v1/{engine}/tts",
                    get(StatusCode::OK).post(StatusCode::OK),
                )
                .route("/api/v1/{engine}/speakers", get(StatusCode::OK))
                .route("/api/v1/tts", get(StatusCode::OK).post(StatusCode::OK))
                .route(LEGACY_SPEAKERS_PATH, get(StatusCode::OK))
                .route("/audio/{engine}/{filename}", get(StatusCode::OK)),
        );
        let requests = [
            (Method::GET, "/api/v1/voicevox/tts/?text=test"),
            (Method::POST, "/api/v1/voicevox/tts/?text=test"),
            (Method::GET, "/api/v1/voicevox/speakers/"),
            (Method::GET, "/api/v1/tts/?text=test"),
            (Method::POST, "/api/v1/tts/?text=test"),
            (Method::GET, "/api/v1/speakers/"),
            (Method::GET, "/audio/voicevox/example.ogg/"),
        ];

        for (method, uri) in requests {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(LEGACY_SPEAKERS_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/speakers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn audio_idは64文字の16進数だけを許可する() {
        let id = make_audio_id("aivisspeech", "1", "text");
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
            public_audio_url(
                "https://tts.example.com",
                "aivisspeech",
                "audio-id",
                &SpeakerAttribution::License("CC0".to_owned()),
            ),
            "https://tts.example.com/audio/aivisspeech/audio-id.ogg?license=CC0"
        );
        assert_eq!(
            public_audio_url(
                "https://tts.example.com",
                "voicevox",
                "audio-id",
                &SpeakerAttribution::Credit("VOICEVOX:ずんだもん".to_owned()),
            ),
            "https://tts.example.com/audio/voicevox/audio-id.ogg?credit=VOICEVOX%3A%E3%81%9A%E3%82%93%E3%81%A0%E3%82%82%E3%82%93"
        );
    }

    #[test]
    fn 利用情報はurlのクエリとしてエンコードする() {
        assert_eq!(
            attribution_query(&SpeakerAttribution::License(
                "Aivis Common Model License (ACML) 1.0 & CC0".to_owned()
            )),
            "license=Aivis+Common+Model+License+%28ACML%29+1.0+%26+CC0"
        );
        assert_eq!(
            attribution_query(&SpeakerAttribution::Credit(
                "VOICEVOX:ずんだもん".to_owned()
            )),
            "credit=VOICEVOX%3A%E3%81%9A%E3%82%93%E3%81%A0%E3%82%82%E3%82%93"
        );
    }

    #[test]
    fn 日本語と記号を含むクエリを復元する() {
        let uri: Uri = "/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF+%26+%E3%81%8A%E3%81%AF%E3%82%88%E3%81%86&speaker=3"
            .parse()
            .unwrap();
        let Query(request) = Query::<TtsRequest>::try_from_uri(&uri).unwrap();

        assert_eq!(request.text, "こんにちは & おはよう");
        assert_eq!(request.speaker.as_deref(), Some("3"));
    }

    #[test]
    fn speakerの省略と未知値には既定値を使う() {
        let speakers = HashMap::from([("3".to_owned(), ()), ("7".to_owned(), ())]);

        assert_eq!(resolve_speaker_id(&speakers, "3", Some("7")), "7");
        assert_eq!(resolve_speaker_id(&speakers, "3", None), "3");
        assert_eq!(resolve_speaker_id(&speakers, "3", Some("999")), "3");
    }

    #[test]
    fn textのない旧json形式はクエリとして受理しない() {
        let uri: Uri = "/tts".parse().unwrap();
        assert!(Query::<TtsRequest>::try_from_uri(&uri).is_err());
    }

    #[tokio::test]
    async fn 成功応答はurlだけのプレーンテキストにする() {
        let url = "https://tts.example.com/audio/example.ogg?license=CC0";
        let response = plain_text_url_response(url.to_owned());

        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body, url);
    }

    #[test]
    fn すべての応答を検索インデックス対象外にする() {
        let response = with_noindex_header(Response::new(Body::empty()));

        assert_eq!(response.headers()["x-robots-tag"], "noindex");
    }
}
