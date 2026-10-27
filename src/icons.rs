//! Inline SVG icons, path data from Tabler Icons (MIT License,
//! Copyright (c) 2020-2024 Paweł Kuna, https://tabler.io/icons). Bundled, no CDN.

use leptos::prelude::*;

pub const ADJUSTMENTS: &str = r#"<path d="M14 6m-2 0a2 2 0 1 0 4 0a2 2 0 1 0 -4 0"/><path d="M4 6l8 0"/><path d="M16 6l4 0"/><path d="M8 12m-2 0a2 2 0 1 0 4 0a2 2 0 1 0 -4 0"/><path d="M4 12l2 0"/><path d="M10 12l10 0"/><path d="M17 18m-2 0a2 2 0 1 0 4 0a2 2 0 1 0 -4 0"/><path d="M4 18l11 0"/><path d="M19 18l1 0"/>"#;
pub const FILE_IMPORT: &str = r#"<path d="M14 3v4a1 1 0 0 0 1 1h4"/><path d="M5 13v-8a2 2 0 0 1 2 -2h7l5 5v11a2 2 0 0 1 -2 2h-5.5m-9.5 -2h7m-3 -3l3 3l-3 3"/>"#;
pub const PLAY: &str = r#"<path d="M6 4v16a1 1 0 0 0 1.524 .852l13 -8a1 1 0 0 0 0 -1.704l-13 -8a1 1 0 0 0 -1.524 .852z" fill="currentColor" stroke="none"/>"#;
pub const PAUSE: &str = r#"<path d="M9 4h-2a2 2 0 0 0 -2 2v12a2 2 0 0 0 2 2h2a2 2 0 0 0 2 -2v-12a2 2 0 0 0 -2 -2z" fill="currentColor" stroke="none"/><path d="M17 4h-2a2 2 0 0 0 -2 2v12a2 2 0 0 0 2 2h2a2 2 0 0 0 2 -2v-12a2 2 0 0 0 -2 -2z" fill="currentColor" stroke="none"/>"#;
pub const STOP: &str = r#"<path d="M5 7a2 2 0 0 1 2 -2h10a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2z"/>"#;
pub const MINIMIZE: &str = r#"<path d="M6 12h12"/>"#;
pub const MAXIMIZE: &str = r#"<path d="M6 8a2 2 0 0 1 2 -2h8a2 2 0 0 1 2 2v8a2 2 0 0 1 -2 2h-8a2 2 0 0 1 -2 -2z"/>"#;
pub const CLOSE: &str = r#"<path d="M18 6l-12 12"/><path d="M6 6l12 12"/>"#;
pub const SPINNER: &str = r#"<path d="M12 3a9 9 0 1 0 9 9"/>"#;
pub const VOLUME: &str = r#"<path d="M15 8a5 5 0 0 1 0 8"/><path d="M17.7 5a9 9 0 0 1 0 14"/><path d="M6 15h-2a1 1 0 0 1 -1 -1v-4a1 1 0 0 1 1 -1h2l3.5 -4.5a.8 .8 0 0 1 1.5 .5v14a.8 .8 0 0 1 -1.5 .5l-3.5 -4.5"/>"#;
pub const DOWNLOAD: &str =
    r#"<path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2 -2v-2"/><path d="M7 11l5 5l5 -5"/><path d="M12 4l0 12"/>"#;
pub const CURSOR_TEXT: &str = r#"<path d="M10 12h4"/><path d="M9 4a3 3 0 0 1 3 3v10a3 3 0 0 1 -3 3"/><path d="M15 4a3 3 0 0 0 -3 3v10a3 3 0 0 0 3 3"/>"#;

/// A 24x24 stroke icon; decorative (`aria-hidden`), label the parent control.
#[component]
pub fn Icon(
    paths: &'static str,
    #[prop(default = 18)] size: u32,
    #[prop(optional)] class: &'static str,
) -> impl IntoView {
    view! {
        <svg
            class=format!("icon {class}")
            width=size
            height=size
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            inner_html=paths
        ></svg>
    }
}
