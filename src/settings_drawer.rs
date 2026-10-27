//! Settings sheet sliding in from the right: Stimme, Wiedergabe, Auswahl,
//! Darstellung, Export. Closes with X, Escape (see `MainApp`) or a click outside.

use leptos::ev;
use leptos::prelude::*;
use openreader_api::{parse_hex_color, OcrEngineKind, ReaderSettings, TtsEngine};
use wasm_bindgen::JsCast;

use crate::icons::{self, Icon};
use crate::logic::{self, BubbleColor, Theme};
use crate::state::{AppState, ExportState};
use crate::tauri;

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
                <AboutSection />
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
    let clear_primary = Memo::new(move |_| {
        state
            .settings
            .with(|s| s.as_ref().map(|s| s.clear_primary_after_capture).unwrap_or(false))
    });
    let disabled = move || state.settings.with(Option::is_none);
    let source_note = Memo::new(move |_| state.health.with(|health| logic::selection_source_note(health.as_ref())));

    view! {
        <section class="section">
            <h3>"Auswahl"</h3>
            {move || source_note.get().map(|note| view! { <p class="field-note selection-source">{note}</p> })}
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
            <label class="switch-row">
                <span>"Markierung nach dem Erfassen leeren"</span>
                <input
                    class="switch clear-primary"
                    type="checkbox"
                    role="switch"
                    disabled=disabled
                    prop:checked=move || clear_primary.get()
                    on:change=move |event| {
                        let checked = input_checked(&event);
                        state.update_settings(move |mut settings| {
                            settings.clear_primary_after_capture = checked;
                            settings
                        });
                    }
                />
            </label>
            <p class="field-note">
                "Verhindert versehentliches Einfügen mit der mittleren Maustaste. \
                 Manche Programme heben danach die farbige Markierung auf."
            </p>
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
    let bubble = Memo::new(move |_| {
        state
            .settings
            .with(|s| s.as_ref().map(ReaderSettings::overlay_bubble_px).unwrap_or(48))
    });
    let accent = Memo::new(move |_| state.settings.with(|s| s.as_ref().and_then(|s| s.accent_color.clone())));
    let bubble_choice = Memo::new(move |_| {
        state
            .settings
            .with(|s| s.as_ref().map(BubbleColor::of).unwrap_or(BubbleColor::Text))
    });
    let bubble_custom = Memo::new(move |_| {
        state.settings.with(|s| {
            s.as_ref()
                .and_then(|s| s.bubble_color.clone())
                .filter(|color| parse_hex_color(color).is_some())
                .unwrap_or_else(|| "#4a8fdc".to_string())
        })
    });
    let disabled = move || state.settings.with(Option::is_none);
    let set_accent = move |color: Option<String>| {
        state.update_settings(move |mut settings| {
            settings.accent_color = color;
            settings
        })
    };
    let set_bubble = move |color: Option<String>| {
        state.update_settings(move |mut settings| {
            settings.bubble_color = color;
            settings
        })
    };
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
            <div class="field">
                <span>"Akzentfarbe"</span>
                <div class="swatches" role="group" aria-label="Akzentfarbe">
                    {logic::ACCENT_PRESETS
                        .into_iter()
                        .map(|(name, color)| {
                            let value = color.map(str::to_string);
                            let pressed = value.clone();
                            view! {
                                <button
                                    class="swatch"
                                    title=name
                                    aria-label=name
                                    disabled=disabled
                                    aria-pressed=move || (accent.get() == pressed).to_string()
                                    style:background=logic::preset_swatch(color)
                                    on:click=move |_| set_accent(value.clone())
                                ></button>
                            }
                        })
                        .collect_view()}
                    <label class="swatch custom" title="Eigene Farbe">
                        <input
                            class="accent-picker"
                            type="color"
                            aria-label="Eigene Akzentfarbe"
                            disabled=disabled
                            prop:value=move || accent.get().unwrap_or_else(|| "#d98e2b".to_string())
                            on:input=move |event| {
                                let color = input_value(&event);
                                if parse_hex_color(&color).is_some() {
                                    set_accent(Some(color));
                                }
                            }
                        />
                    </label>
                </div>
            </div>
            <div class="field">
                <span>"Farbe von Vorlese-Blase und Abspielknopf"</span>
                <div class="bubble-color-row">
                    <div class="segmented" role="group" aria-label="Farbe der Vorlese-Blase">
                        {BubbleColor::ALL
                            .into_iter()
                            .map(|choice| {
                                view! {
                                    <button
                                        class="segment"
                                        disabled=disabled
                                        aria-pressed=move || (bubble_choice.get() == choice).to_string()
                                        on:click=move |_| set_bubble(match choice {
                                            BubbleColor::Text => None,
                                            BubbleColor::Accent => Some("accent".to_string()),
                                            BubbleColor::Custom => Some(bubble_custom.get_untracked()),
                                        })
                                    >
                                        {choice.label()}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                    <Show when=move || bubble_choice.get() == BubbleColor::Custom>
                        <label class="swatch custom" title="Eigene Blasenfarbe">
                            <input
                                class="bubble-picker"
                                type="color"
                                aria-label="Eigene Blasenfarbe"
                                prop:value=move || bubble_custom.get()
                                on:input=move |event| {
                                    let color = input_value(&event);
                                    if parse_hex_color(&color).is_some() {
                                        set_bubble(Some(color));
                                    }
                                }
                            />
                        </label>
                    </Show>
                </div>
            </div>
            <label class="field range">
                <span class="range-label">
                    "Größe der Vorlese-Blase" <output>{move || format!("{} px", bubble.get())}</output>
                </span>
                <div class="bubble-size-row">
                    <input
                        type="range"
                        min=ReaderSettings::OVERLAY_BUBBLE_MIN
                        max=ReaderSettings::OVERLAY_BUBBLE_MAX
                        step="2"
                        disabled=move || state.settings.with(Option::is_none)
                        prop:value=move || bubble.get().to_string()
                        on:input=move |event| {
                            if let Ok(value) = input_value(&event).parse::<i64>() {
                                state.update_settings(move |mut settings| {
                                    settings.overlay_bubble_size = value;
                                    settings
                                });
                            }
                        }
                    />
                    <span class="bubble-preview-slot" aria-hidden="true">
                        <span
                            class="bubble-preview"
                            style:width=move || format!("{}px", bubble.get())
                            style:height=move || format!("{}px", bubble.get())
                        >
                            <Icon paths=icons::PLAY size=15 />
                        </span>
                    </span>
                </div>
            </label>
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

#[component]
fn AboutSection() -> impl IntoView {
    let state = expect_context::<AppState>();
    let current = Memo::new(move |_| {
        state
            .update
            .with(|info| info.as_ref().map(|info| info.current.clone()))
            .or_else(|| state.health.with(|health| health.as_ref().map(|health| health.version.clone())))
            .unwrap_or_default()
    });
    let check_updates = Memo::new(move |_| {
        state
            .settings
            .with(|s| s.as_ref().map(|s| s.check_updates).unwrap_or(true))
    });
    let release = Memo::new(move |_| state.update.with(|info| info.as_ref().and_then(|info| info.available.clone())));

    view! {
        <section class="section about-section">
            <h3>"Über OpenReader"</h3>
            <p class="about-version">{move || format!("Version {}", current.get())}</p>
            <p class="update-status" class:available=move || release.with(Option::is_some)>
                {move || state.update.with(|info| logic::update_status(info.as_ref()))}
            </p>
            {move || {
                release.get().map(|release| {
                    let url = release.url.clone();
                    view! {
                        {(!release.notes.is_empty()).then(|| view! { <p class="update-notes">{release.notes.clone()}</p> })}
                        <button class="secondary-button" on:click=move |_| tauri::open_release_page(url.clone())>
                            "Release-Seite öffnen"
                        </button>
                    }
                })
            }}
            <label class="switch-row">
                <span>"Automatisch nach Updates suchen"</span>
                <input
                    class="switch check-updates"
                    type="checkbox"
                    role="switch"
                    disabled=move || state.settings.with(Option::is_none)
                    prop:checked=move || check_updates.get()
                    on:change=move |event| {
                        let checked = input_checked(&event);
                        state.update_settings(move |mut settings| {
                            settings.check_updates = checked;
                            settings
                        });
                    }
                />
            </label>
            <button
                class="secondary-button check-now"
                disabled=move || state.update_checking.get()
                on:click=move |_| state.check_update()
            >
                {move || if state.update_checking.get() { "Prüfe …" } else { "Jetzt prüfen" }}
            </button>
            <p class="field-note">
                "Fragt einmal am Tag bei GitHub nach einer neuen Version. Dabei wird nur die "
                "Versionsnummer übertragen, keine Daten über dich oder deine Texte."
            </p>
        </section>
    }
}
