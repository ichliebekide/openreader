//! Reactive state of the main window and the actions that change it.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use openreader_api::{
    HealthResponse, PlaybackState, ReaderEvent, ReaderSettings, SelectionEvent, TtsStatusResponse, UpdateInfo,
    VoiceProfile,
};

use wasm_bindgen::JsCast;

use crate::api;
use crate::logic::{self, PlaybackModel, Theme};
use crate::tauri;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub text: String,
    pub char_count: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SaveState {
    #[default]
    Idle,
    Saving,
    Saved,
    Error,
}

impl SaveState {
    pub fn label(self) -> &'static str {
        match self {
            SaveState::Idle => "",
            SaveState::Saving => "Speichert …",
            SaveState::Saved => "Gespeichert",
            SaveState::Error => "Nicht gespeichert",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ExportState {
    #[default]
    Idle,
    Running,
    Done(String),
    Failed(String),
}

/// Non-reactive bookkeeping (React refs in the old UI).
#[derive(Default)]
struct Bookkeeping {
    playback_revision: u64,
    snapshot_pending: bool,
    pending_settings: Option<ReaderSettings>,
    save_in_flight: bool,
    save_revision: u64,
    save_timer: Option<TimeoutHandle>,
    feedback_timer: Option<TimeoutHandle>,
}

#[derive(Clone, Copy)]
pub struct AppState {
    pub health: RwSignal<Option<HealthResponse>>,
    /// Result of the backend's last update check.
    pub update: RwSignal<Option<UpdateInfo>>,
    pub update_checking: RwSignal<bool>,
    pub settings: RwSignal<Option<ReaderSettings>>,
    pub tts_status: RwSignal<Option<TtsStatusResponse>>,
    pub save_state: RwSignal<SaveState>,
    pub selection: RwSignal<Selection>,
    pub playback: RwSignal<PlaybackModel>,
    pub import_error: RwSignal<Option<String>>,
    pub importing: RwSignal<bool>,
    pub control_busy: RwSignal<bool>,
    pub drawer_open: RwSignal<bool>,
    pub theme: RwSignal<Theme>,
    pub export_state: RwSignal<ExportState>,
    books: StoredValue<Bookkeeping, LocalStorage>,
}

const THEME_KEY: &str = "openreader.theme";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

impl AppState {
    pub fn new() -> Self {
        let theme = local_storage()
            .and_then(|storage| storage.get_item(THEME_KEY).ok().flatten())
            .map(|value| Theme::parse(&value))
            .unwrap_or(Theme::System);
        let state = Self {
            health: RwSignal::new(None),
            update: RwSignal::new(None),
            update_checking: RwSignal::new(false),
            settings: RwSignal::new(None),
            tts_status: RwSignal::new(None),
            save_state: RwSignal::new(SaveState::Idle),
            selection: RwSignal::new(Selection::default()),
            playback: RwSignal::new(PlaybackModel::default()),
            import_error: RwSignal::new(None),
            importing: RwSignal::new(false),
            control_busy: RwSignal::new(false),
            drawer_open: RwSignal::new(false),
            theme: RwSignal::new(theme),
            export_state: RwSignal::new(ExportState::Idle),
            books: StoredValue::new_local(Bookkeeping::default()),
        };
        apply_theme(theme);
        state
    }

    /// Start polling, load the settings and follow the event stream.
    pub fn start(self) {
        self.refresh();
        let _ = set_interval_with_handle(move || self.refresh(), Duration::from_millis(1600));

        spawn_local(async move {
            if let Ok(settings) = api::settings().await {
                self.settings.set(Some(settings));
            }
        });

        // The backend checks GitHub once a day; the UI only reads its result.
        self.load_update();
        let _ = set_interval_with_handle(move || self.load_update(), Duration::from_secs(30 * 60));

        spawn_local(api::run_event_stream(api::EventHandlers {
            on_open: Box::new(move || {
                spawn_local(async move {
                    if let Ok(snapshot) = api::reading_snapshot().await {
                        self.apply_playback(&snapshot.state, Some(&snapshot.text));
                    }
                });
            }),
            on_event: Box::new(move |event| match event {
                ReaderEvent::Selection(selection) => self.on_selection(selection),
                ReaderEvent::Playback(event) => self.apply_playback(&event.state, event.text.as_deref()),
                ReaderEvent::Fallback(fallback) => self.playback.update(|model| model.notice = Some(fallback.message)),
            }),
        }));
    }

    fn load_update(self) {
        spawn_local(async move {
            if let Ok(info) = api::update_info().await {
                self.update.set(Some(info));
            }
        });
    }

    /// "Jetzt pruefen" in the settings drawer.
    pub fn check_update(self) {
        self.update_checking.set(true);
        spawn_local(async move {
            if let Ok(info) = api::check_update().await {
                self.update.set(Some(info));
            }
            self.update_checking.set(false);
        });
    }

    fn refresh(self) {
        spawn_local(async move {
            self.health.set(api::health().await.ok());
        });
        spawn_local(async move {
            self.tts_status.set(api::tts_status().await.ok());
        });
        let revision = self.books.with_value(|books| books.playback_revision);
        spawn_local(async move {
            let result = api::playback_state().await;
            let current = self.books.with_value(|books| books.playback_revision);
            if self.control_busy.get_untracked() || revision != current {
                return;
            }
            match result {
                Ok(state) => self.apply_playback(&state, None),
                Err(_) => self.playback.update(PlaybackModel::reset_connection),
            }
        });
    }

    fn on_selection(self, event: SelectionEvent) {
        let x = event.cursor_x.unwrap_or(48);
        let y = event.cursor_y.unwrap_or(48);
        self.selection.set(Selection {
            text: event.text,
            char_count: event.char_count,
        });
        let size = self
            .settings
            .with_untracked(|settings| settings.as_ref().map(ReaderSettings::overlay_bubble_px))
            .unwrap_or(48);
        spawn_local(async move {
            let position = tauri::OverlayPosition { x: x + 18, y: y + 18, size };
            let _ = tauri::invoke("show_overlay", &position).await;
        });
    }

    pub fn apply_playback(self, state: &PlaybackState, text: Option<&str>) {
        let mut needs_snapshot = false;
        self.playback.update(|model| needs_snapshot = model.apply(state, text));
        let already_pending = self.books.with_value(|books| books.snapshot_pending);
        if needs_snapshot && !already_pending {
            self.books.update_value(|books| books.snapshot_pending = true);
            spawn_local(async move {
                if let Ok(snapshot) = api::reading_snapshot().await {
                    self.apply_playback(&snapshot.state, Some(&snapshot.text));
                }
                self.books.update_value(|books| books.snapshot_pending = false);
            });
        }
    }

    // -------------------------------------------------------------- playback

    async fn playback_command(self, command: impl std::future::Future<Output = Result<(), String>>) {
        if self.control_busy.get_untracked() {
            return;
        }
        self.control_busy.set(true);
        self.books.update_value(|books| books.playback_revision += 1);
        self.playback.update(|model| model.error = None);
        if let Err(error) = command.await {
            let message = if error.trim().is_empty() {
                "Sprachausgabe fehlgeschlagen.".into()
            } else {
                error
            };
            self.playback.update(|model| model.error = Some(message));
        }
        self.books.update_value(|books| books.playback_revision += 1);
        self.control_busy.set(false);
    }

    pub fn selected_profile(self) -> Option<VoiceProfile> {
        self.settings
            .with(|settings| settings.as_ref().and_then(logic::selected_profile).cloned())
    }

    fn selected_profile_id(self) -> Option<String> {
        self.settings.with_untracked(|settings| {
            settings
                .as_ref()
                .and_then(logic::selected_profile)
                .map(|profile| profile.id.clone())
        })
    }

    async fn snapshot(self) -> Result<(), String> {
        let snapshot = api::reading_snapshot().await?;
        self.apply_playback(&snapshot.state, Some(&snapshot.text));
        Ok(())
    }

    pub fn speak_selection(self) {
        let text = self
            .selection
            .with_untracked(|selection| selection.text.trim().to_string());
        if text.is_empty() || self.playback.with_untracked(|model| model.speaking) {
            return;
        }
        let profile = self.selected_profile_id();
        spawn_local(self.playback_command(async move {
            api::speak(&text, profile).await?;
            self.snapshot().await
        }));
    }

    pub fn test_tts(self) {
        if self.playback.with_untracked(|model| model.speaking) {
            return;
        }
        let profile = self.selected_profile_id();
        spawn_local(self.playback_command(async move {
            api::test_tts(profile).await?;
            self.snapshot().await
        }));
    }

    pub fn stop(self) {
        spawn_local(self.playback_command(async move {
            let state = api::stop().await?;
            self.apply_playback(&state, None);
            Ok(())
        }));
    }

    pub fn toggle_pause(self) {
        let paused = self.playback.with_untracked(|model| model.paused);
        spawn_local(self.playback_command(async move {
            let state = if paused {
                api::resume().await?
            } else {
                api::pause().await?
            };
            self.apply_playback(&state, None);
            Ok(())
        }));
    }

    /// Main button: read the selection, or pause/resume the running request.
    pub fn primary_action(self) {
        if self.playback.with_untracked(|model| model.speaking) {
            self.toggle_pause();
        } else {
            self.speak_selection();
        }
    }

    // -------------------------------------------------------------- import / export

    pub fn import_file(self, file: web_sys::File) {
        self.importing.set(true);
        self.import_error.set(None);
        spawn_local(async move {
            match api::extract_document(&file).await {
                Ok(document) => self.selection.set(Selection {
                    text: document.text,
                    char_count: document.char_count,
                }),
                Err(error) => self.import_error.set(Some(error)),
            }
            self.importing.set(false);
        });
    }

    /// The text that is shown (and would be exported): reading text or selection.
    pub fn current_text(self) -> String {
        let reading = self
            .playback
            .with_untracked(|model| model.shows_reading_text().then(|| model.text.clone()));
        reading.unwrap_or_else(|| self.selection.with_untracked(|selection| selection.text.clone()))
    }

    pub fn export(self, path: Option<String>) {
        let text = self.current_text();
        if text.trim().is_empty() || self.export_state.get_untracked() == ExportState::Running {
            return;
        }
        let profile = self.selected_profile_id();
        self.export_state.set(ExportState::Running);
        spawn_local(async move {
            let result = api::export(text.trim(), profile, path).await;
            self.export_state.set(match result {
                Ok(response) => ExportState::Done(response.path),
                Err(error) => ExportState::Failed(error),
            });
        });
    }

    // -------------------------------------------------------------- settings

    /// Apply a change locally at once and save it (debounced, one request at a time).
    pub fn update_settings(self, update: impl FnOnce(ReaderSettings) -> ReaderSettings) {
        let Some(current) = self.settings.get_untracked() else {
            return;
        };
        let next = update(current);
        self.settings.set(Some(next.clone()));
        self.save_state.set(SaveState::Saving);
        self.books.update_value(|books| {
            books.pending_settings = Some(next);
            books.save_revision += 1;
            if let Some(timer) = books.feedback_timer.take() {
                timer.clear();
            }
            if let Some(timer) = books.save_timer.take() {
                timer.clear();
            }
        });
        let timer = set_timeout_with_handle(
            move || {
                self.books.update_value(|books| books.save_timer = None);
                spawn_local(self.flush_settings());
            },
            Duration::from_millis(100),
        )
        .ok();
        self.books.update_value(|books| books.save_timer = timer);
    }

    async fn flush_settings(self) {
        let next = self.books.try_update_value(|books| {
            if books.save_in_flight {
                return None;
            }
            let next = books.pending_settings.take()?;
            books.save_in_flight = true;
            Some((next, books.save_revision))
        });
        let Some(Some((next, revision))) = next else {
            return;
        };
        let result = api::update_settings(&next).await;
        let status = api::tts_status().await;
        let current = self
            .books
            .with_value(|books| (books.save_revision, books.pending_settings.is_some()));
        match result {
            Ok(saved) if current == (revision, false) => {
                self.settings.set(Some(saved));
                if let Ok(status) = status {
                    self.tts_status.set(Some(status));
                }
                self.save_state.set(SaveState::Saved);
                let timer = set_timeout_with_handle(
                    move || self.save_state.set(SaveState::Idle),
                    Duration::from_millis(1300),
                )
                .ok();
                self.books.update_value(|books| books.feedback_timer = timer);
            }
            Ok(_) => {}
            Err(_) if current.0 == revision => self.save_state.set(SaveState::Error),
            Err(_) => {}
        }
        let again = self.books.try_update_value(|books| {
            books.save_in_flight = false;
            books.pending_settings.is_some()
        });
        if again == Some(true) {
            spawn_local(self.flush_settings());
        }
    }

    pub fn update_profile(self, update: impl FnOnce(&mut VoiceProfile) + 'static) {
        self.update_settings(|settings| logic::update_selected_profile(settings, update));
    }

    // -------------------------------------------------------------- theme

    pub fn set_theme(self, theme: Theme) {
        self.theme.set(theme);
        apply_theme(theme);
        if let Some(storage) = local_storage() {
            let _ = storage.set_item(THEME_KEY, theme.as_str());
        }
    }
}

/// Colour settings as inline custom properties on `<html>` (inline styles win
/// over the light/dark rules of the stylesheet).
pub fn apply_colors(settings: Option<&ReaderSettings>) {
    let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    else {
        return;
    };
    let style = root.unchecked_ref::<web_sys::HtmlElement>().style();
    for (property, value) in logic::color_properties(settings) {
        let _ = match value {
            Some(value) => style.set_property(property, &value),
            None => style.remove_property(property).map(|_| ()),
        };
    }
}

/// `data-theme` on `<html>` overrides `prefers-color-scheme`; absent = system.
pub fn apply_theme(theme: Theme) {
    let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    else {
        return;
    };
    let _ = match theme {
        Theme::System => root.remove_attribute("data-theme"),
        Theme::Light | Theme::Dark => root.set_attribute("data-theme", theme.as_str()),
    };
}
