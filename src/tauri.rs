//! Minimal bindings to the Tauri JavaScript API (`window.__TAURI__`, enabled
//! with `app.withGlobalTauri`). Outside Tauri (plain browser, UI checks) every
//! call is a no-op, so the UI also runs against an `external` backend.

use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    async fn tauri_listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> Result<JsValue, JsValue>;

    type TauriWindow;

    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "window"], js_name = getCurrentWindow)]
    fn current_window() -> Result<TauriWindow, JsValue>;

    #[wasm_bindgen(method, catch, js_name = startDragging)]
    async fn start_dragging(this: &TauriWindow) -> Result<JsValue, JsValue>;
}

/// `true` inside the Tauri WebView.
pub fn available() -> bool {
    web_sys::window()
        .map(|window| js_sys::Reflect::has(&window, &JsValue::from_str("__TAURI__")).unwrap_or(false))
        .unwrap_or(false)
}

/// Invoke a Tauri command; errors are logged, never fatal for the UI.
pub async fn invoke<A: Serialize>(command: &str, args: &A) -> Result<(), String> {
    if !available() {
        return Ok(());
    }
    let args = serde_wasm_bindgen::to_value(args).map_err(|error| error.to_string())?;
    tauri_invoke(command, args)
        .await
        .map(|_| ())
        .map_err(|error| error.as_string().unwrap_or_else(|| format!("{error:?}")))
}

#[derive(Serialize)]
pub struct NoArgs {}

#[derive(Serialize)]
pub struct OverlayPosition {
    pub x: i32,
    pub y: i32,
    /// Visible button diameter (`ReaderSettings::overlay_bubble_px`).
    pub size: u32,
}

#[derive(Serialize)]
pub struct ReleasePage {
    pub url: String,
}

/// Open a release page in the browser (outside Tauri: a new tab).
pub fn open_release_page(url: String) {
    if available() {
        leptos::task::spawn_local(async move {
            let _ = invoke("open_release_page", &ReleasePage { url }).await;
        });
    } else if let Some(window) = web_sys::window() {
        let _ = window.open_with_url_and_target(&url, "_blank");
    }
}

#[derive(Serialize)]
pub struct OverlayInput {
    pub enabled: bool,
}

/// Subscribe to a Tauri event with a boolean payload for the page's lifetime.
pub async fn listen_bool(event: &str, mut handler: impl FnMut(bool) + 'static) {
    listen(event, move |payload| handler(payload.as_bool().unwrap_or(false))).await;
}

/// Subscribe to a Tauri event with a numeric payload for the page's lifetime.
pub async fn listen_u32(event: &str, mut handler: impl FnMut(u32) + 'static) {
    listen(event, move |payload| {
        if let Some(value) = payload.as_f64() {
            handler(value as u32);
        }
    })
    .await;
}

async fn listen(event: &str, mut handler: impl FnMut(JsValue) + 'static) {
    if !available() {
        return;
    }
    let callback = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let payload = js_sys::Reflect::get(&event, &JsValue::from_str("payload"))
            .unwrap_or(JsValue::UNDEFINED);
        handler(payload);
    });
    if tauri_listen(event, &callback).await.is_ok() {
        // The listener lives as long as the window; Tauri keeps calling it.
        callback.forget();
    }
}

/// Let the compositor move the undecorated main window.
pub fn start_window_drag() {
    if !available() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async {
        if let Ok(window) = current_window() {
            let _ = window.start_dragging().await;
        }
    });
}
