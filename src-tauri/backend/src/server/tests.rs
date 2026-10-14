//! Port of `backend/tests/test_reader_api.py` plus fallback visibility.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tokio::sync::Notify;
use tower::ServiceExt;

use super::*;

#[derive(Default)]
struct ControlledTts {
    paused: AtomicBool,
    started: Notify,
    started_flag: AtomicBool,
    calls: Mutex<Vec<String>>,
    fail: Mutex<Option<String>>,
    slow_stop: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
}

#[async_trait]
impl Speech for ControlledTts {
    async fn speak(&self, text: &str, _profile: Option<&str>) -> Result<()> {
        self.calls.lock().unwrap().push(text.to_string());
        self.started_flag.store(true, Ordering::SeqCst);
        self.started.notify_waiters();
        if let Some(message) = self.fail.lock().unwrap().clone() {
            anyhow::bail!(message);
        }
        std::future::pending::<()>().await;
        Ok(())
    }
    async fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    async fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    async fn stop(&self) {
        let gate = self.slow_stop.lock().unwrap().clone();
        if let Some((entered, allow)) = gate {
            let allowed = allow.notified();
            entered.notify_waiters();
            allowed.await;
        }
        self.paused.store(false, Ordering::SeqCst);
    }
    fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    fn active_language(&self) -> String {
        "German".into()
    }
    fn status(&self) -> TtsStatusResponse {
        TtsStatusResponse {
            preferred_engine: TtsEngine::Piper,
            engines: vec![],
            fallback_order: vec![TtsEngine::Piper],
        }
    }
    async fn export_wav(&self, _: &str, output: &Path, _: Option<&str>) -> Result<PathBuf> {
        Ok(output.to_path_buf())
    }
}

impl ControlledTts {
    async fn wait_started(&self) {
        for _ in 0..200 {
            if self.started_flag.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("speech did not start");
    }
}

fn prepare() -> (Arc<ReaderApp>, Arc<ControlledTts>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let tts = Arc::new(ControlledTts::default());
    let shared = tts.clone();
    let factory: SpeechFactory = Arc::new(move |_, _| shared.clone() as Arc<dyn Speech>);
    let app = ReaderApp::new(SettingsStore::new(Some(dir.path().join("settings.json"))), factory);
    (app, tts, dir)
}

async fn call(app: &Arc<ReaderApp>, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = router(app.clone()).oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

#[tokio::test]
async fn api_pause_resume_stop_and_duplicate_requests() {
    let (app, tts, _dir) = prepare();
    let requests = (0..10).map(|_| call(&app, "POST", "/api/speak", Some(serde_json::json!({"text": "Hallo"}))));
    let results = futures::future::join_all(requests).await;
    tts.wait_started().await;
    let started = results.iter().filter(|(_, body)| body["started"] == true).count();
    assert_eq!(started, 1);
    assert_eq!(*tts.calls.lock().unwrap(), ["Hallo"]);
    assert_eq!(call(&app, "POST", "/api/tts/pause", None).await.1["paused"], true);
    assert_eq!(call(&app, "GET", "/api/tts/playback", None).await.1["speaking"], true);
    let again = call(&app, "POST", "/api/speak", Some(serde_json::json!({"text": "Nochmals"}))).await.1;
    assert_eq!(again["started"], false);
    assert_eq!(call(&app, "POST", "/api/tts/resume", None).await.1["paused"], false);
    let state = call(&app, "POST", "/api/tts/stop", None).await.1;
    assert_eq!((state["speaking"].clone(), state["paused"].clone()), (false.into(), false.into()));
    assert_eq!(call(&app, "POST", "/api/tts/pause", None).await.1["paused"], false);
    let new = call(&app, "POST", "/api/speak", Some(serde_json::json!({"text": "Neuer Text"}))).await.1;
    assert_eq!(new["started"], true);
    app.stop_speech().await;
}

#[tokio::test]
async fn stop_and_start_are_serialized() {
    let (app, tts, _dir) = prepare();
    let entered = Arc::new(Notify::new());
    let allow = Arc::new(Notify::new());
    app.start_speech(SpeakRequest { text: "Alt".into(), profile_id: None, export_path: None }).await;
    tts.wait_started().await;
    *tts.slow_stop.lock().unwrap() = Some((entered.clone(), allow.clone()));
    let entered_wait = entered.notified();
    let stopping = tokio::spawn({
        let app = app.clone();
        async move { app.stop_speech().await }
    });
    entered_wait.await;
    let starting = tokio::spawn({
        let app = app.clone();
        async move { app.start_speech(SpeakRequest { text: "Neu".into(), profile_id: None, export_path: None }).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!starting.is_finished());
    tts.slow_stop.lock().unwrap().take();
    allow.notify_waiters();
    stopping.await.unwrap();
    assert!(starting.await.unwrap().started);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(*tts.calls.lock().unwrap(), ["Alt", "Neu"]);
    app.stop_speech().await;
}

#[tokio::test]
async fn reading_snapshot_progress_and_lifecycle() {
    let (app, tts, _dir) = prepare();
    app.start_speech(SpeakRequest { text: "Hallo Welt".into(), profile_id: None, export_path: None }).await;
    tts.wait_started().await;
    let initial = app.reading_snapshot();
    assert_eq!(initial.text, "Hallo Welt");
    assert_eq!(initial.state.word_start, None);
    app.word_progress(Some(WordBoundary { start: 6, end: 10, offset: 1.0, duration: 0.4 }));
    let state = app.playback_state();
    assert_eq!(state.session_id, initial.state.session_id);
    assert!(state.revision > initial.state.revision);
    assert_eq!((state.word_start, state.word_end), (Some(6), Some(10)));
    // Polling does not resend the entire book.
    assert!(serde_json::to_value(&state).unwrap().get("text").is_none());
    app.pause().await;
    let paused = app.reading_snapshot();
    assert!(paused.state.paused && paused.state.word_start == Some(6) && paused.text == initial.text);
    app.stop_speech().await;
    let stopped = app.reading_snapshot();
    assert!(!stopped.state.speaking && stopped.text.is_empty() && stopped.state.word_start.is_none());
    app.start_speech(SpeakRequest { text: "Neuer Text".into(), profile_id: None, export_path: None }).await;
    let new = app.reading_snapshot();
    assert_ne!(new.state.session_id, initial.state.session_id);
    assert!(new.state.revision > stopped.state.revision);
    assert_eq!(new.state.word_start, None);
    app.stop_speech().await;
}

#[tokio::test]
async fn playback_errors_are_observable_and_reset_on_new_request() {
    let (app, tts, _dir) = prepare();
    *tts.fail.lock().unwrap() = Some("Audio connection lost".into());
    app.start_speech(SpeakRequest { text: "Text".into(), profile_id: None, export_path: None }).await;
    for _ in 0..100 {
        if !app.is_speaking() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let state = app.playback_state();
    assert!(!state.speaking);
    assert_eq!(state.error.as_deref(), Some("Audio connection lost"));
    *tts.fail.lock().unwrap() = None;
    app.start_speech(SpeakRequest { text: "Nochmal".into(), profile_id: None, export_path: None }).await;
    assert_eq!(app.playback_state().error, None);
    app.stop_speech().await;
}

#[tokio::test]
async fn fallback_is_published_and_visible_in_playback_state() {
    let (app, _tts, _dir) = prepare();
    let mut events = app.hub.subscribe();
    app.start_speech(SpeakRequest { text: "Hallo".into(), profile_id: None, export_path: None }).await;
    let _ = events.next().await; // playback start
    app.fallback(FallbackNotice {
        failed: edge_seraphina_profile(),
        error: "HTTP 403".into(),
        fallback: piper_profile("piper-de-mls", "Piper Deutsch MLS", "de_DE-mls-medium", 0.95),
    });
    let ReaderEvent::Fallback(event) = events.next().await else {
        panic!("expected fallback event");
    };
    assert_eq!(event.failed_engine, TtsEngine::Edge);
    assert_eq!(event.fallback_profile_id, "piper-de-mls");
    assert!(event.message.contains("Microsoft Seraphina") && event.message.contains("HTTP 403"));
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "tts_fallback");
    let ReaderEvent::Playback(playback) = events.next().await else {
        panic!("expected playback event");
    };
    assert_eq!(playback.state.notice.as_deref(), Some(event.message.as_str()));
    // A new request clears the notice.
    app.stop_speech().await;
    app.start_speech(SpeakRequest { text: "Neu".into(), profile_id: None, export_path: None }).await;
    assert_eq!(app.playback_state().notice, None);
    app.stop_speech().await;
}

struct BlockingExtractor {
    entered: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
    paths: Mutex<Vec<PathBuf>>,
}

impl DocumentExtractor for BlockingExtractor {
    fn extract_text(&self, path: &Path, language: &str) -> Result<String> {
        assert_eq!(language, "German");
        assert_eq!(std::fs::read(path).unwrap(), b"test");
        self.paths.lock().unwrap().push(path.to_path_buf());
        self.entered.wait();
        self.release.wait();
        Ok("Dokumenttext".into())
    }
}

fn multipart(name: &str, data: &[u8]) -> Request<Body> {
    let boundary = "openreaderboundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method("POST")
        .uri("/api/documents/extract")
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_import_does_not_block_health_and_cleans_up() {
    let (app, _tts, _dir) = prepare();
    let extractor = Arc::new(BlockingExtractor {
        entered: Arc::new(std::sync::Barrier::new(2)),
        release: Arc::new(std::sync::Barrier::new(2)),
        paths: Mutex::new(Vec::new()),
    });
    app.set_documents(extractor.clone());
    let request = tokio::spawn(router(app.clone()).oneshot(multipart("file.txt", b"test")));
    let entered = extractor.entered.clone();
    tokio::task::spawn_blocking(move || entered.wait()).await.unwrap();
    let health = tokio::time::timeout(Duration::from_millis(500), call(&app, "GET", "/api/health", None))
        .await
        .unwrap();
    assert_eq!(health.0, StatusCode::OK);
    assert_eq!(health.1["backend"], "rust");
    let release = extractor.release.clone();
    tokio::task::spawn_blocking(move || release.wait()).await.unwrap();
    let response = request.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body, serde_json::json!({"text": "Dokumenttext", "char_count": 12}));
    let path = extractor.paths.lock().unwrap()[0].clone();
    assert!(!path.exists() && !path.parent().unwrap().exists());
}

struct FailingExtractor {
    message: &'static str,
    paths: Mutex<Vec<PathBuf>>,
}

impl DocumentExtractor for FailingExtractor {
    fn extract_text(&self, path: &Path, _: &str) -> Result<String> {
        self.paths.lock().unwrap().push(path.to_path_buf());
        if self.message.is_empty() {
            Ok(String::new())
        } else {
            anyhow::bail!(self.message)
        }
    }
}

#[tokio::test]
async fn import_errors_and_empty_documents_return_422() {
    for message in ["OCR-Sprachdaten fehlen: deu", ""] {
        let (app, _tts, _dir) = prepare();
        let extractor = Arc::new(FailingExtractor {
            message,
            paths: Mutex::new(Vec::new()),
        });
        app.set_documents(extractor.clone());
        let response = router(app.clone()).oneshot(multipart("file.png", b"image")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(!body["detail"].as_str().unwrap().is_empty());
        let path = extractor.paths.lock().unwrap()[0].clone();
        assert!(path.to_string_lossy().ends_with("document.png"));
        assert!(!path.exists());
    }
}

#[tokio::test]
async fn settings_roundtrip_and_cors() {
    let (app, _tts, dir) = prepare();
    let (status, settings) = call(&app, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    let mut settings: ReaderSettings = serde_json::from_value(settings).unwrap();
    settings.min_selection_chars = 9;
    settings.voice_profiles.retain(|profile| profile.id != "piper-de-mls");
    let (status, saved) = call(&app, "PUT", "/api/settings", Some(serde_json::to_value(&settings).unwrap())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["min_selection_chars"], 9);
    // Deleted default profiles come back, exactly like the Python store.
    assert!(saved["voice_profiles"].as_array().unwrap().iter().any(|p| p["id"] == "piper-de-mls"));
    let written = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    assert!(written.contains("\"min_selection_chars\": 9"));
    let (status, _) = call(&app, "PUT", "/api/settings", Some(serde_json::json!({"backend_engine": "qwen"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/speak")
                .header("origin", "tauri://localhost")
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["access-control-allow-origin"], "tauri://localhost");
    assert!(is_local_origin("http://localhost:5173"));
    assert!(is_local_origin("https://127.0.0.1"));
    assert!(!is_local_origin("http://evil.example"));
    assert!(!is_local_origin("http://localhost.evil:80"));
}

#[test]
fn background_server_serves_and_shuts_down() {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut handle = spawn_background(ServerConfig {
        host: "127.0.0.1".into(),
        port,
        selection_helper: None,
        settings_path: Some(dir.path().join("settings.json")),
        desktop_integration: false,
    })
    .unwrap();
    let mut body = String::new();
    for _ in 0..100 {
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            stream
                .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            stream.read_to_string(&mut body).unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(body.contains("\"backend\":\"rust\""));
    handle.shutdown();
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}
