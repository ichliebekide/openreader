//! OpenReader UI (Leptos, client-side rendered, built with Trunk).
//!
//! One bundle serves both Tauri windows: `index.html` is the main window and
//! `index.html#/overlay` the transparent overlay bubble.

mod api;
mod icons;
mod logic;
mod overlay;
mod reader;
mod settings_drawer;
mod state;
mod tauri;

fn is_overlay() -> bool {
    web_sys::window()
        .and_then(|window| window.location().hash().ok())
        .is_some_and(|hash| hash.contains("overlay"))
}

fn main() {
    console_error_panic_hook::set_once();
    if is_overlay() {
        if let Some(root) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.document_element())
        {
            let _ = root.class_list().add_1("overlay");
        }
        leptos::mount::mount_to_body(overlay::OverlayApp);
    } else {
        leptos::mount::mount_to_body(reader::MainApp);
    }
}
