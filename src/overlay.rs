//! Overlay window (`index.html#/overlay`): one round "read aloud" button.
//!
//! Window geometry, focus and input behaviour are owned by the Tauri shell
//! (`src-tauri/src/main.rs`, `overlay_input.rs`): the window is a square of
//! face + 24 logical pixels and only the inscribed circle accepts input. The
//! button fills that circle; the visible face (`overlay_bubble_size`, default
//! 48px, sent by the shell as `overlay-size`) has a soft shadow for legibility
//! on any desktop.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use openreader_api::ReaderEvent;

use crate::api;
use crate::icons::{self, Icon};
use crate::tauri;

/// Same as the Rust shell's hide timer for a shown overlay.
const OVERLAY_TIMEOUT: Duration = Duration::from_millis(4800);

#[component]
pub fn OverlayApp() -> impl IntoView {
    let text = RwSignal::new(String::new());
    let selection_sequence = RwSignal::new(0u64);
    let visible = RwSignal::new(false);
    let busy = RwSignal::new(false);

    spawn_local(api::run_event_stream(api::EventHandlers {
        on_open: Box::new(|| {}),
        on_event: Box::new(move |event| {
            if let ReaderEvent::Selection(selection) = event {
                text.set(selection.text);
                selection_sequence.update(|sequence| *sequence += 1);
            }
        }),
    }));

    spawn_local(tauri::listen_u32("overlay-size", |face| {
        if let Some(root) = document().document_element() {
            let _ = root
                .unchecked_ref::<web_sys::HtmlElement>()
                .style()
                .set_property("--bubble-face", &format!("{face}px"));
        }
    }));

    // The bubble colour lives in the settings; refresh it whenever the bubble shows.
    let refresh_colors = || {
        spawn_local(async {
            if let Ok(settings) = api::settings().await {
                crate::state::apply_colors(Some(&settings));
            }
        })
    };
    refresh_colors();

    spawn_local(tauri::listen_bool("overlay-visibility", move |shown| {
        visible.set(shown);
        if shown {
            refresh_colors();
            // Enable the input region only once the shown frame is painted.
            request_animation_frame(move || {
                request_animation_frame(move || {
                    spawn_local(async {
                        let _ = tauri::invoke("set_overlay_input", &tauri::OverlayInput { enabled: true }).await;
                    });
                });
            });
        }
    }));

    // Hide after the timeout; a new selection while visible restarts it.
    let timer = StoredValue::new_local(None::<TimeoutHandle>);
    Effect::new(move |_| {
        selection_sequence.track();
        let shown = visible.get();
        if let Some(handle) = timer.try_update_value(Option::take).flatten() {
            handle.clear();
        }
        if shown {
            let handle = set_timeout_with_handle(
                || {
                    spawn_local(async {
                        let _ = tauri::invoke("hide_overlay", &tauri::NoArgs {}).await;
                    });
                },
                OVERLAY_TIMEOUT,
            )
            .ok();
            timer.set_value(handle);
        }
    });

    let on_click = move |_| {
        let selected = text.get_untracked().trim().to_string();
        if selected.is_empty() || busy.get_untracked() || !visible.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            if api::speak(&selected, None).await.is_ok() {
                let _ = tauri::invoke("hide_overlay", &tauri::NoArgs {}).await;
            }
            busy.set(false);
        });
    };

    view! {
        <button
            class="overlay-bubble"
            class:busy=move || busy.get()
            class:hidden=move || !visible.get()
            aria-label="Markierten Text vorlesen"
            title="Markierten Text vorlesen"
            disabled=move || busy.get() || !visible.get()
            on:click=on_click
        >
            <span class="bubble-face">
                {move || {
                    if busy.get() {
                        view! { <Icon paths=icons::SPINNER size=22 class="spin" /> }.into_any()
                    } else {
                        view! { <Icon paths=icons::PLAY size=21 class="play-glyph" /> }.into_any()
                    }
                }}
            </span>
        </button>
    }
}
