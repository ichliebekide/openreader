//! Settings sheet sliding in from the right: Stimme, Wiedergabe, Auswahl,
//! Darstellung, Export. Closes with X, Escape (see `MainApp`) or a click outside.

use leptos::ev;
use leptos::prelude::*;
use openreader_api::{OcrEngineKind, TtsEngine};
use wasm_bindgen::JsCast;

use crate::icons::{self, Icon};
use crate::logic::{self, Theme};
use crate::state::{AppState, ExportState};

fn input_value(event: &ev::Event) -> String {
    event
        .target()
        .and_then(|target| {
            target
                .dyn_ref::<web_sys::HtmlInputElement>()
                .map(|input| input.value())
                .or_else(|| {
                    target
                        .dyn_ref::<web_sys::HtmlSelectElement>()
                        .map(|select| select.value())
                })
        })
        .unwrap_or_default()
}

fn input_checked(event: &ev::Event) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
        .is_some_and(|input| input.checked())
}

#[component]
pub fn SettingsDrawer() -> impl IntoView {
    let state = expect_context::<AppState>();
    let open = state.drawer_open;
    let close_button = NodeRef::<leptos::html::Button>::new();

    Effect::new(move |previous: Option<bool>| {
        let is_open = open.get();
        if is_open && previous == Some(false) {
            request_animation_frame(move || {
                if let Some(button) = close_button.get_untracked() {
                    let _ = button.focus();
                }
            });
        }
        is_open
    });

    view! {
        <div class="scrim" class:open=move || open.get() on:click=move |_| open.set(false) aria-hidden="true"></div>
        <aside
            class="drawer"
            class:open=move || open.get()
            role="dialog"
            aria-modal="true"
            aria-labelledby="drawer-title"
            aria-hidden=move || (!open.get()).to_string()
            inert=move || !open.get()
        >
            <header class="drawer-header">
                <h2 id="drawer-title">"Einstellungen"</h2>
                <span class="save-state" role="status">{move || state.save_state.get().label()}</span>
                <button
                    node_ref=close_button
                    class="icon-button"
                    aria-label="Schließen"
                    title="Schließen"
                    on:click=move |_| open.set(false)
                >
                    <Icon paths=icons::CLOSE />
                </button>
            </header>
            <div class="drawer-body">
                <VoiceSection />
                <PlaybackSection />
                <SelectionSection />
                <DocumentsSection />
                <AppearanceSection />
                <ExportSection />
            </div>
        </aside>
    }
}

#[component]
fn VoiceSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    let engine = Memo::new(move |_| {
        state
            .settings
            .with(|settings| settings.as_ref().map(|s| s.backend_engine))
    });
    let profiles = Memo::new(move |_| {
        state
            .settings
            .with(|settings| settings.as_ref().map(logic::engine_profiles).unwrap_or_default())
    });
    let selected_id = Memo::new(move |_| state.selected_profile().map(|profile| profile.id));
    let speaking = Memo::new(move |_| state.playback.with(|model| model.speaking));

    view! {
        <section class="section">
            <h3>"Stimme"</h3>
            <label class="field">
                <span>"TTS-Engine"</span>
                <span class="select">
                <select
                    class="engine-select"
                    disabled=move || engine.get().is_none()
                    on:change=move |event| {
                        if let Some(engine) = TtsEngine::parse(&input_value(&event)) {
                            state.update_settings(|settings| logic::with_engine(settings, engine));
                        }
                    }
                >
                    {TtsEngine::ALL
                        .into_iter()
                        .map(|option| {
                            view! {
                                <option value=option.as_str() selected=move || engine.get() == Some(option)>
                                    {logic::engine_option_label(option)}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
                </span>
            </label>
            <label class="field">
                <span>"Stimmprofil"</span>
                <span class="select">
                <select
                    class="profile-select"
                    disabled=move || profiles.with(Vec::is_empty)
                    on:change=move |event| {
                        let id = input_value(&event);
                        state.update_settings(move |settings| logic::with_profile(settings, &id));
                    }
                >
                    <For each=move || profiles.get() key=|profile| profile.id.clone() let:profile>
                        {
                            let id = profile.id.clone();
                            view! {
                                <option
                                    value=profile.id.clone()
                                    selected=move || selected_id.get().as_deref() == Some(id.as_str())
                                >
                                    {profile.label.clone()}
                                </option>
                            }
                        }
                    </For>
                </select>
                </span>
            </label>
            <ul class="engine-status" aria-label="Engine-Status">
                {move || {
                    state
                        .tts_status
                        .get()
                        .map(|status| status.engines)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|engine| {
                            let ready = engine.available && engine.configured;
                            let detail = if ready {
                                "bereit".to_string()
                            } else if engine.detail.trim().is_empty() {
                                "nicht bereit".to_string()
                            } else {
                                engine.detail.clone()
                            };
                            let title = detail.clone();
                            view! {
                                <li class="status-row">
                                    <span class=if ready { "status-dot ok" } else { "status-dot off" }></span>
                                    <span class="status-name">{logic::engine_label(engine.engine)}</span>
                                    <span class="status-detail" title=title>{detail}</span>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
            <button class="secondary-button" disabled=move || speaking.get() on:click=move |_| state.test_tts()>
                <Icon paths=icons::VOLUME size=16 />
                {move || if speaking.get() { "TTS läuft" } else { "TTS testen" }}
            </button>
        </section>
    }
}

#[component]
fn PlaybackSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    let profile = Memo::new(move |_| state.selected_profile());
    let speed = Memo::new(move |_| {
        profile.with(|profile| {
            profile
                .as_ref()
                .map(|profile| logic::speed_from_length_scale(profile.length_scale))
                .unwrap_or(1.0)
        })
    });
    let volume = Memo::new(move |_| profile.with(|profile| profile.as_ref().map(|p| p.volume).unwrap_or(1.0)));

    view! {
        <section class="section">
            <h3>"Wiedergabe"</h3>
            <label class="field range">
                <span class="range-label">
                    "Tempo" <output>{move || logic::format_speed(speed.get())}</output>
                </span>
                <input
                    type="range"
                    min=logic::SPEED_MIN
                    max=logic::SPEED_MAX
                    step="0.05"
                    disabled=move || profile.with(Option::is_none)
                    prop:value=move || format!("{:.2}", speed.get())
                    on:input=move |event| {
                        if let Ok(value) = input_value(&event).parse::<f64>() {
                            state.update_profile(move |profile| profile.length_scale = logic::length_scale_from_speed(value));
                        }
                    }
                />
            </label>
            <label class="field range">
                <span class="range-label">
                    "Lautstärke" <output>{move || format!("{:.0} %", volume.get() * 100.0)}</output>
                </span>
                <input
                    type="range"
                    min="0.25"
                    max="1.8"
                    step="0.05"
                    disabled=move || profile.with(Option::is_none)
                    prop:value=move || format!("{:.2}", volume.get())
                    on:input=move |event| {
                        if let Ok(value) = input_value(&event).parse::<f64>() {
                            state.update_profile(move |profile| profile.volume = value);
                        }
                    }
                />
            </label>
        </section>
    }
}

#[component]
fn SelectionSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    let min_chars = Memo::new(move |_| {
        state
            .settings
            .with(|s| s.as_ref().map(|s| s.min_selection_chars).unwrap_or(3))
    });
    let ignore_terminals = Memo::new(move |_| {
        state
            .settings
            .with(|s| s.as_ref().map(|s| s.ignore_terminal_windows).unwrap_or(true))
    });
    let disabled = move || state.settings.with(Option::is_none);

    view! {
        <section class="section">
            <h3>"Auswahl"</h3>
            <label class="field inline">
                <span>"Minimale Textlänge"</span>
                <input
                    class="number-input"
                    type="number"
                    min="1"
                    disabled=disabled
                    prop:value=move || min_chars.get().to_string()
                    on:change=move |event| {
                        if let Ok(value) = input_value(&event).trim().parse::<i64>() {
                            let value = value.max(1);
                            state.update_settings(move |mut settings| {
                                settings.min_selection_chars = value;
                                settings
                            });
                        }
                    }
                />
            </label>
            <label class="switch-row">
                <span>"Terminals ignorieren"</span>
                <input
                    class="switch"
                    type="checkbox"
                    role="switch"
                    disabled=disabled
                    prop:checked=move || ignore_terminals.get()
                    on:change=move |event| {
                        let checked = input_checked(&event);
                        state.update_settings(move |mut settings| {
                            settings.ignore_terminal_windows = checked;
                            settings
                        });
                    }
                />
            </label>
        </section>
    }
}

#[component]
fn DocumentsSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    let ocr = Memo::new(move |_| state.settings.with(|s| s.as_ref().map(|s| s.ocr_engine)));

    view! {
        <section class="section">
            <h3>"Dokumente"</h3>
            <label class="field">
                <span>"Texterkennung (OCR) für Bilder"</span>
                <span class="select">
                    <select
                        class="ocr-select"
                        disabled=move || ocr.get().is_none()
                        on:change=move |event| {
                            if let Some(engine) = OcrEngineKind::parse(&input_value(&event)) {
                                state.update_settings(move |mut settings| {
                                    settings.ocr_engine = engine;
                                    settings
                                });
                            }
                        }
                    >
                        {OcrEngineKind::ALL
                            .into_iter()
                            .map(|option| {
                                view! {
                                    <option value=option.as_str() selected=move || ocr.get() == Some(option)>
                                        {logic::ocr_engine_label(option)}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </span>
            </label>
            <p class="field-note">
                {move || ocr.get().map(logic::ocr_engine_hint).unwrap_or_default()}
            </p>
        </section>
    }
}

#[component]
fn AppearanceSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    view! {
        <section class="section">
            <h3>"Darstellung"</h3>
            <div class="segmented" role="group" aria-label="Farbschema">
                {Theme::ALL
                    .into_iter()
                    .map(|theme| {
                        view! {
                            <button
                                class="segment"
                                aria-pressed=move || (state.theme.get() == theme).to_string()
                                on:click=move |_| state.set_theme(theme)
                            >
                                {theme.label()}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        </section>
    }
}

#[component]
fn ExportSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    let path = RwSignal::new(String::new());
    let has_text = Memo::new(move |_| {
        let reading = state.playback.with(|model| model.shows_reading_text());
        reading || state.selection.with(|selection| !selection.text.trim().is_empty())
    });
    let running = Memo::new(move |_| state.export_state.get() == ExportState::Running);

    view! {
        <section class="section">
            <h3>"Export"</h3>
            <label class="field">
                <span>"Zieldatei (WAV)"</span>
                <input
                    class="text-input"
                    type="text"
                    placeholder="Standard: ~/OpenReader.wav"
                    spellcheck="false"
                    prop:value=move || path.get()
                    on:input=move |event| path.set(input_value(&event))
                />
            </label>
            <button
                class="secondary-button"
                disabled=move || !has_text.get() || running.get()
                on:click=move |_| {
                    let target = path.get_untracked().trim().to_string();
                    state.export((!target.is_empty()).then_some(target));
                }
            >
                {move || {
                    if running.get() {
                        view! { <Icon paths=icons::SPINNER size=16 class="spin" /> }.into_any()
                    } else {
                        view! { <Icon paths=icons::DOWNLOAD size=16 /> }.into_any()
                    }
                }}
                {move || if running.get() { "Exportiert …" } else { "Aktuellen Text exportieren" }}
            </button>
            {move || match state.export_state.get() {
                ExportState::Done(path) => {
                    view! { <p class="field-note" role="status">"Gespeichert: " {path}</p> }.into_any()
                }
                ExportState::Failed(error) => view! { <p class="field-note error" role="alert">{error}</p> }.into_any(),
                _ => ().into_any(),
            }}
        </section>
    }
}
