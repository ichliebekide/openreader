//! Main window ("Fokus"): top bar, reading text, message line, player bar.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::icons::{self, Icon};
use crate::logic::{self, Readiness};
use crate::settings_drawer::SettingsDrawer;
use crate::state::AppState;
use crate::tauri;

pub const IMPORT_ACCEPT: &str = ".pdf,.epub,.txt,.md,.html,.htm,.xhtml,.png,.jpg,.jpeg,.tif,.tiff,.webp";

#[component]
pub fn MainApp() -> impl IntoView {
    let state = AppState::new();
    state.start();
    provide_context(state);

    let _escape = window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" && state.drawer_open.get_untracked() {
            state.drawer_open.set(false);
        }
    });

    view! {
        <main class="app">
            <TopBar />
            <ReadingView />
            <MessageLine />
            <PlayerBar />
            <SettingsDrawer />
        </main>
    }
}

fn state() -> AppState {
    expect_context::<AppState>()
}

#[component]
fn TopBar() -> impl IntoView {
    let state = state();
    let file_input = NodeRef::<html::Input>::new();

    let char_count = Memo::new(move |_| {
        let reading = state
            .playback
            .with(|model| model.shows_reading_text().then(|| logic::char_count(&model.text)));
        reading.unwrap_or_else(|| state.selection.with(|selection| selection.char_count))
    });
    let voice = Memo::new(move |_| {
        state
            .selected_profile()
            .map(|profile| logic::short_voice_label(&profile.label))
    });
    let readiness = Memo::new(move |_| state.health.with(|health| Readiness::of(health.as_ref())));

    let on_mousedown = move |event: ev::MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let on_control = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|element| element.closest("button, input, select, label, a").ok().flatten())
            .is_some();
        if !on_control {
            tauri::start_window_drag();
        }
    };

    let on_file = move |event: ev::Event| {
        let Some(input) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let file = input.files().and_then(|files| files.get(0));
        input.set_value("");
        if let Some(file) = file {
            state.import_file(file);
        }
    };

    view! {
        <header class="topbar" data-tauri-drag-region="" on:mousedown=on_mousedown>
            <div class="meta" data-tauri-drag-region="">
                <span
                    class=move || readiness.get().class()
                    title=move || readiness.get().label()
                    role="img"
                    aria-label=move || readiness.get().label()
                ></span>
                <span class="meta-text" data-tauri-drag-region="">
                    {move || format!("{} Zeichen", char_count.get())}
                    {move || voice.get().map(|voice| format!(" · {voice}"))}
                </span>
            </div>
            <div class="topbar-actions">
                <button
                    class="icon-button"
                    title="Datei öffnen"
                    aria-label="Datei öffnen"
                    aria-busy=move || state.importing.get().to_string()
                    disabled=move || state.importing.get()
                    on:click=move |_| {
                        if let Some(input) = file_input.get() {
                            input.click();
                        }
                    }
                >
                    {move || {
                        if state.importing.get() {
                            view! { <Icon paths=icons::SPINNER class="spin" /> }.into_any()
                        } else {
                            view! { <Icon paths=icons::FILE_IMPORT /> }.into_any()
                        }
                    }}
                </button>
                <input
                    node_ref=file_input
                    class="file-input"
                    type="file"
                    hidden
                    accept=IMPORT_ACCEPT
                    on:change=on_file
                />
                <button
                    class="icon-button"
                    title="Einstellungen"
                    aria-label="Einstellungen"
                    aria-expanded=move || state.drawer_open.get().to_string()
                    on:click=move |_| state.drawer_open.update(|open| *open = !*open)
                >
                    <Icon paths=icons::ADJUSTMENTS />
                </button>
            </div>
        </header>
    }
}

/// Text shown in the reading column.
#[derive(Clone, Debug, PartialEq)]
enum Display {
    Empty,
    Text(String),
}

#[component]
fn ReadingView() -> impl IntoView {
    let state = state();
    let scroller = NodeRef::<html::Div>::new();

    let display = Memo::new(move |_| {
        let reading = state
            .playback
            .with(|model| model.shows_reading_text().then(|| model.text.clone()));
        let text = reading.unwrap_or_else(|| state.selection.with(|selection| selection.text.clone()));
        if text.trim().is_empty() {
            Display::Empty
        } else {
            Display::Text(text)
        }
    });
    let word = Memo::new(move |_| {
        state
            .playback
            .with(|model| if model.shows_reading_text() { model.word } else { None })
    });
    // (before, word, after); an invalid range shows the plain text.
    let parts = Memo::new(move |_| match display.get() {
        Display::Empty => (String::new(), None, String::new()),
        Display::Text(text) => match logic::split_highlight(&text, word.get()) {
            Some((before, word, after)) => (before.to_string(), Some(word.to_string()), after.to_string()),
            None => (text, None, String::new()),
        },
    });

    // Keep the spoken word inside the visible area (12px margin).
    Effect::new(move |_| {
        if parts.with(|parts| parts.1.is_none()) {
            return;
        }
        request_animation_frame(move || {
            let Some(container) = scroller.get_untracked() else {
                return;
            };
            let Ok(Some(mark)) = container.query_selector("mark.spoken-word") else {
                return;
            };
            let bounds = container.get_bounding_client_rect();
            let target = mark.get_bounding_client_rect();
            if target.top() < bounds.top() + 12.0 {
                container.set_scroll_top(container.scroll_top() - (bounds.top() + 12.0 - target.top()) as i32);
            } else if target.bottom() > bounds.bottom() - 12.0 {
                container.set_scroll_top(container.scroll_top() + (target.bottom() - bounds.bottom() + 12.0) as i32);
            }
        });
    });

    view! {
        <div class="reading-scroll" node_ref=scroller>
            <Show
                when=move || display.with(|display| *display != Display::Empty)
                fallback=|| {
                    view! {
                        <div class="empty-state">
                            <Icon paths=icons::CURSOR_TEXT size=22 />
                            <p class="empty-title">"Markiere Text in einer beliebigen App"</p>
                            <p class="empty-hint">
                                "OpenReader zeigt die Auswahl hier an und liest sie vor. "
                                "Du kannst auch eine Datei öffnen."
                            </p>
                        </div>
                    }
                }
            >
                <article class="reading-text" lang="de">
                    {move || parts.with(|parts| parts.0.clone())}
                    <Show when=move || parts.with(|parts| parts.1.is_some())>
                        <mark class="spoken-word">{move || parts.with(|parts| parts.1.clone().unwrap_or_default())}</mark>
                    </Show>
                    {move || parts.with(|parts| parts.2.clone())}
                </article>
            </Show>
        </div>
    }
}

#[component]
fn MessageLine() -> impl IntoView {
    let state = state();
    let error = Memo::new(move |_| {
        state
            .import_error
            .get()
            .or_else(|| state.playback.with(|model| model.error.clone()))
    });
    let notice = Memo::new(move |_| state.playback.with(|model| model.notice.clone()));

    view! {
        <div class="message-line">
            {move || match (error.get(), notice.get()) {
                (Some(error), _) => {
                    let title = error.clone();
                    view! { <p class="message error" role="alert" title=title>{error}</p> }.into_any()
                }
                (None, Some(notice)) => {
                    let title = notice.clone();
                    view! { <p class="message notice" role="status" title=title>{notice}</p> }.into_any()
                }
                (None, None) => ().into_any(),
            }}
        </div>
    }
}

#[component]
fn PlayerBar() -> impl IntoView {
    let state = state();
    let speaking = Memo::new(move |_| state.playback.with(|model| model.speaking));
    let paused = Memo::new(move |_| state.playback.with(|model| model.paused));
    let has_selection = Memo::new(move |_| state.selection.with(|selection| !selection.text.trim().is_empty()));
    let progress = Memo::new(move |_| {
        state.playback.with(|model| {
            if model.shows_reading_text() {
                model.progress()
            } else {
                0.0
            }
        })
    });
    let speed = Memo::new(move |_| {
        state
            .selected_profile()
            .map(|profile| logic::speed_from_length_scale(profile.length_scale))
            .unwrap_or(1.0)
    });

    let primary_label = move || {
        if paused.get() {
            "Fortsetzen"
        } else if speaking.get() {
            "Pausieren"
        } else {
            "Vorlesen"
        }
    };

    view! {
        <footer class="player">
            <button
                class="play-button"
                aria-label=primary_label
                title=primary_label
                disabled=move || state.control_busy.get() || (!speaking.get() && !has_selection.get())
                on:click=move |_| state.primary_action()
            >
                {move || {
                    if speaking.get() && !paused.get() {
                        view! { <Icon paths=icons::PAUSE size=18 /> }.into_any()
                    } else {
                        view! { <Icon paths=icons::PLAY size=18 class="play-glyph" /> }.into_any()
                    }
                }}
            </button>
            <div
                class="progress"
                role="progressbar"
                aria-label="Lesefortschritt"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow=move || format!("{:.0}", progress.get() * 100.0)
            >
                <div class="progress-fill" style:width=move || format!("{:.2}%", progress.get() * 100.0)></div>
            </div>
            <button
                class="speed-button"
                title=move || {
                    if speaking.get() { "Tempo gilt ab dem nächsten Vorlesen" } else { "Tempo ändern" }
                }
                aria-label=move || format!("Tempo {}", logic::format_speed(speed.get()))
                disabled=move || state.settings.with(Option::is_none) || speaking.get()
                on:click=move |_| {
                    let next = logic::next_speed(speed.get_untracked());
                    state.update_profile(move |profile| profile.length_scale = logic::length_scale_from_speed(next));
                }
            >
                {move || logic::format_speed(speed.get())}
            </button>
            <button
                class="icon-button"
                aria-label="Stoppen"
                title="Stoppen"
                disabled=move || !speaking.get() || state.control_busy.get()
                on:click=move |_| state.stop()
            >
                <Icon paths=icons::STOP />
            </button>
        </footer>
    }
}
