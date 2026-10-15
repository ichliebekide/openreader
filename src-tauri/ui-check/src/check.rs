//! The scenario: settings, import, timed word highlight, pause freeze,
//! reading snapshot after reload, stop, import errors, fallback notice,
//! settings drawer, themes, overlay bubble and responsive layout.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use headless_chrome::protocol::cdp::{Emulation, Page};
use headless_chrome::{Browser, LaunchOptions, Tab};
use serde_json::Value;

use crate::fixture::Fixture;

const ERROR_PROBE: &str = r#"
window.__uiErrors = [];
addEventListener("error", (event) => window.__uiErrors.push(String(event.message)));
addEventListener("unhandledrejection", (event) => window.__uiErrors.push(String(event.reason)));
"#;

struct Page2<'a> {
    tab: Arc<Tab>,
    screenshots: &'a Path,
}

impl Page2<'_> {
    fn eval(&self, expression: &str) -> Result<Value> {
        // Objects come back as remote references; transfer everything as JSON.
        let wrapped = format!("(async () => JSON.stringify([await ({expression})]))()");
        let result = self
            .tab
            .evaluate(&wrapped, true)
            .with_context(|| format!("evaluate {expression}"))?;
        match result.value {
            Some(Value::String(json)) => match serde_json::from_str::<Value>(&json)? {
                Value::Array(mut values) if !values.is_empty() => Ok(values.swap_remove(0)),
                other => bail!("unexpected result of {expression}: {other}"),
            },
            _ => bail!("{expression} threw: {:?}", result.description),
        }
    }

    fn text(&self, expression: &str) -> Result<String> {
        Ok(self.eval(expression)?.as_str().unwrap_or_default().to_string())
    }

    fn wait(&self, what: &str, condition: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.eval(&format!("Boolean({condition})"))? == Value::Bool(true) {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {what} ({condition})");
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn click(&self, selector: &str) -> Result<()> {
        self.tab
            .find_element(selector)
            .with_context(|| format!("find {selector}"))?
            .click()
            .with_context(|| format!("click {selector}"))?;
        Ok(())
    }

    fn import(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let bytes = serde_json::to_string(bytes)?;
        self.eval(&format!(
            r#"(() => {{
                const input = document.querySelector('input[type="file"]');
                const transfer = new DataTransfer();
                transfer.items.add(new File([new Uint8Array({bytes})], {name:?}, {{ type: "text/plain" }}));
                input.files = transfer.files;
                input.dispatchEvent(new Event("change", {{ bubbles: true }}));
                return true;
            }})()"#
        ))?;
        Ok(())
    }

    fn viewport(&self, width: u32, height: u32) -> Result<()> {
        self.tab.call_method(Emulation::SetDeviceMetricsOverride {
            width,
            height,
            device_scale_factor: 1.0,
            mobile: false,
            scale: None,
            screen_width: None,
            screen_height: None,
            position_x: None,
            position_y: None,
            dont_set_visible_size: None,
            screen_orientation: None,
            viewport: None,
            display_feature: None,
            device_posture: None,
        })?;
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    fn color_scheme(&self, scheme: &str) -> Result<()> {
        self.tab.call_method(Emulation::SetEmulatedMedia {
            media: None,
            features: Some(vec![Emulation::MediaFeature {
                name: "prefers-color-scheme".into(),
                value: scheme.into(),
            }]),
        })?;
        std::thread::sleep(Duration::from_millis(250));
        Ok(())
    }

    fn screenshot(&self, name: &str) -> Result<()> {
        // Let transitions (drawer, progress) settle.
        std::thread::sleep(Duration::from_millis(320));
        let png = self
            .tab
            .capture_screenshot(Page::CaptureScreenshotFormatOption::Png, None, None, true)?;
        std::fs::write(self.screenshots.join(format!("{name}.png")), png)?;
        Ok(())
    }

    fn screenshot_clip(&self, name: &str, x: f64, y: f64, width: f64, height: f64, scale: f64) -> Result<()> {
        std::thread::sleep(Duration::from_millis(250));
        let clip = Page::Viewport {
            x,
            y,
            width,
            height,
            scale,
        };
        let png = self
            .tab
            .capture_screenshot(Page::CaptureScreenshotFormatOption::Png, None, Some(clip), true)?;
        std::fs::write(self.screenshots.join(format!("{name}.png")), png)?;
        Ok(())
    }

    fn goto(&self, url: &str) -> Result<()> {
        self.tab.navigate_to(url)?.wait_until_navigated()?;
        Ok(())
    }

    fn errors(&self) -> Result<Vec<String>> {
        Ok(serde_json::from_value(self.eval("window.__uiErrors || []")?)?)
    }
}

fn launch(chrome: Option<&Path>) -> Result<Browser> {
    let path = chrome
        .map(Path::to_path_buf)
        .or_else(|| {
            [
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/usr/bin/google-chrome",
                "/usr/bin/google-chrome-stable",
            ]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|path| path.exists())
        })
        .ok_or_else(|| anyhow!("no Chrome/Chromium found; pass --chrome or set OPENREADER_CHROMIUM"))?;
    let options = LaunchOptions::default_builder()
        .path(Some(path))
        .headless(true)
        .window_size(Some((1240, 800)))
        .idle_browser_timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| anyhow!("{error}"))?;
    Browser::new(options)
}

fn playback(fixture: &Fixture) -> openreader_api::PlaybackState {
    fixture.app.playback_state()
}

pub fn run(fixture: &Fixture, chrome: Option<&Path>, screenshots: &Path) -> Result<()> {
    let browser = launch(chrome)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(10));
    tab.call_method(Page::AddScriptToEvaluateOnNewDocument {
        source: ERROR_PROBE.into(),
        world_name: None,
        include_command_line_api: None,
        run_immediately: None,
    })?;
    let page = Page2 { tab, screenshots };
    page.viewport(1240, 800)?;
    page.color_scheme("light")?;
    page.goto(&fixture.page_url(""))?;

    // Settings and engine status from the backend.
    page.wait(
        "settings",
        "document.querySelectorAll('.engine-select option').length === 2 && document.querySelector('.profile-select option')",
    )?;
    let engines = page.eval("[...document.querySelectorAll('.engine-select option')].map(o => o.textContent)")?;
    ensure!(
        engines == serde_json::json!(["Microsoft Seraphina", "Piper"]),
        "engine options {engines}"
    );
    let profile = page.text("document.querySelector('.profile-select').selectedOptions[0].textContent")?;
    ensure!(profile == "Microsoft Seraphina", "selected profile {profile}");
    page.wait("engine status", "document.querySelectorAll('.status-row').length === 2")?;
    let names = page.eval("[...document.querySelectorAll('.status-name')].map(o => o.textContent)")?;
    ensure!(
        names == serde_json::json!(["Seraphina", "Piper"]),
        "status rows {names}"
    );
    let details = page.eval("[...document.querySelectorAll('.status-detail')].map(o => o.textContent)")?;
    ensure!(
        details == serde_json::json!(["bereit", "Stimmmodell nicht installiert"]),
        "status details {details}"
    );
    page.wait(
        "status dot",
        "document.querySelector('.topbar .status-dot.ok, .topbar .status-dot.warn')",
    )?;
    page.wait("empty state", "document.querySelector('.empty-state')")?;
    ensure!(
        page.eval("document.querySelector('.play-button').disabled")? == Value::Bool(true),
        "play must be disabled without text"
    );
    page.screenshot("fokus-light-empty")?;

    // A selection from another application.
    fixture.select("Markierter Text aus einer anderen Anwendung.");
    page.wait(
        "selection",
        "document.querySelector('.reading-text')?.textContent === 'Markierter Text aus einer anderen Anwendung.'",
    )?;
    let meta = page.text("document.querySelector('.meta-text').textContent")?;
    ensure!(meta == "44 Zeichen · Seraphina", "meta line {meta:?}");

    // Import and timed word highlight.
    let text = "Erster Satz. Version 1.2.3 bleibt unverändert! "
        .repeat(20)
        .trim()
        .to_string();
    page.import("test.txt", text.as_bytes())?;
    page.wait(
        "imported text",
        &format!("document.querySelector('.reading-text')?.textContent === {text:?}"),
    )?;
    page.click("button[aria-label='Vorlesen']")?;
    page.wait("word highlight", "document.querySelector('mark.spoken-word')")?;
    let word = page.text("document.querySelector('mark.spoken-word').textContent")?;
    ensure!(
        text.split_whitespace().any(|candidate| candidate == word),
        "highlighted {word:?}"
    );
    page.wait(
        "third word",
        "document.querySelector('mark.spoken-word')?.textContent === '1.2.3'",
    )?;
    page.wait(
        "progress",
        "parseFloat(document.querySelector('.progress-fill').style.width) > 0",
    )?;
    page.screenshot("fokus-light-reading")?;

    // Pause freezes the highlight.
    page.click("button[aria-label='Pausieren']")?;
    page.wait(
        "resume button",
        "document.querySelector(\"button[aria-label='Fortsetzen']\")",
    )?;
    ensure!(playback(fixture).paused, "backend not paused");
    std::thread::sleep(Duration::from_millis(150));
    let paused_word = page.text("document.querySelector('mark.spoken-word').textContent")?;
    std::thread::sleep(Duration::from_millis(500));
    ensure!(
        page.text("document.querySelector('mark.spoken-word').textContent")? == paused_word,
        "highlight moved while paused"
    );

    // A newly imported selection must not replace the text being spoken.
    page.import("next.txt", "Andere Auswahl.".as_bytes())?;
    std::thread::sleep(Duration::from_millis(200));
    ensure!(
        page.text("document.querySelector('.reading-text').textContent")? == text,
        "import replaced the spoken text"
    );

    // Reload: the reading snapshot restores text, word and paused state.
    page.tab.reload(false, None)?.wait_until_navigated()?;
    page.wait(
        "resume after reload",
        "document.querySelector(\"button[aria-label='Fortsetzen']\")",
    )?;
    page.wait("word after reload", "document.querySelector('mark.spoken-word')")?;
    ensure!(
        page.text("document.querySelector('mark.spoken-word').textContent")? == paused_word,
        "word after reload"
    );
    ensure!(
        page.text("document.querySelector('.reading-text').textContent")? == text,
        "text after reload"
    );
    page.screenshot("fokus-light-paused")?;

    // Resume, then stop.
    page.click("button[aria-label='Fortsetzen']")?;
    page.wait(
        "pause button",
        "document.querySelector(\"button[aria-label='Pausieren']\")",
    )?;
    page.wait(
        "highlight moves on",
        &format!("document.querySelector('mark.spoken-word')?.textContent !== {paused_word:?}"),
    )?;
    page.click("button[aria-label='Stoppen']")?;
    page.wait(
        "play button",
        "document.querySelector(\"button[aria-label='Vorlesen']\")",
    )?;
    ensure!(!playback(fixture).speaking, "backend still speaking");
    ensure!(
        page.eval("document.querySelectorAll('mark.spoken-word').length")? == 0,
        "highlight after stop"
    );

    // Import errors are shown as a quiet single line.
    page.import("test.txt", text.as_bytes())?;
    page.wait(
        "text again",
        &format!("document.querySelector('.reading-text')?.textContent === {text:?}"),
    )?;
    page.import("broken.txt", &[0xff, 0x41])?;
    page.wait("import error", "document.querySelector('.message-line [role=alert]')")?;
    let error = page.text("document.querySelector('.message-line [role=alert]').textContent")?;
    ensure!(error.contains("UTF-8"), "import error {error:?}");
    page.screenshot("fokus-light-error")?;

    // Fallback notice (no error).
    page.import("test.txt", text.as_bytes())?;
    page.wait("error cleared", "!document.querySelector('.message-line [role=alert]')")?;
    fixture
        .switches
        .fallback_next
        .store(true, std::sync::atomic::Ordering::SeqCst);
    page.click("button[aria-label='Vorlesen']")?;
    page.wait(
        "fallback notice",
        "document.querySelector('.message-line .notice[role=status]')",
    )?;
    let notice = page.text("document.querySelector('.message-line .notice').textContent")?;
    ensure!(
        notice.contains("Es wird Piper Deutsch Thorsten Medium verwendet"),
        "notice {notice:?}"
    );
    page.wait("word with notice", "document.querySelector('mark.spoken-word')")?;
    page.screenshot("fokus-light-notice")?;

    // Settings drawer: open, Escape, click outside.
    page.click("button[aria-label='Einstellungen']")?;
    page.wait("drawer open", "document.querySelector('.drawer.open')")?;
    page.screenshot("fokus-light-settings")?;
    page.tab.press_key("Escape")?;
    page.wait("drawer closed by Escape", "!document.querySelector('.drawer.open')")?;
    page.click("button[aria-label='Einstellungen']")?;
    page.wait("drawer reopened", "document.querySelector('.drawer.open')")?;
    page.eval("document.querySelector('.scrim').click(), true")?;
    page.wait("drawer closed by scrim", "!document.querySelector('.drawer.open')")?;

    // Dark mode follows the system; the manual choice overrides it.
    page.color_scheme("dark")?;
    let dark_bg = page.text("getComputedStyle(document.body).backgroundColor")?;
    ensure!(dark_bg == "rgb(31, 31, 30)", "dark background {dark_bg}");
    page.screenshot("fokus-dark-reading")?;
    page.click("button[aria-label='Einstellungen']")?;
    page.wait("drawer open (dark)", "document.querySelector('.drawer.open')")?;
    page.screenshot("fokus-dark-settings")?;
    page.eval("[...document.querySelectorAll('.segment')].find(b => b.textContent === 'Hell').click(), true")?;
    page.wait("light override", "document.documentElement.dataset.theme === 'light'")?;
    let light_bg = page.text("getComputedStyle(document.body).backgroundColor")?;
    ensure!(light_bg == "rgb(250, 250, 248)", "light override background {light_bg}");
    page.eval("[...document.querySelectorAll('.segment')].find(b => b.textContent === 'System').click(), true")?;
    page.wait("system theme", "!document.documentElement.dataset.theme")?;
    page.tab.press_key("Escape")?;
    page.wait("drawer closed (dark)", "!document.querySelector('.drawer.open')")?;

    // Speed readout cycles the tempo of the active profile (saved to the backend).
    page.click("button[aria-label='Stoppen']")?;
    page.wait("stopped", "document.querySelector(\"button[aria-label='Vorlesen']\")")?;
    page.click(".speed-button")?;
    page.wait(
        "speed 1.1x",
        "document.querySelector('.speed-button').textContent === '1.1x'",
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let settings = fixture.app.settings();
        let profile = settings
            .voice_profiles
            .iter()
            .find(|p| p.id == "edge-de-seraphina")
            .unwrap();
        if (profile.length_scale - 0.909).abs() < 1e-9 {
            break;
        }
        ensure!(Instant::now() < deadline, "tempo not saved: {}", profile.length_scale);
        std::thread::sleep(Duration::from_millis(50));
    }

    // Responsive layout: no horizontal scrolling, controls stay reachable.
    page.color_scheme("light")?;
    for (width, height) in [(1240u32, 800u32), (1040, 660), (390, 844)] {
        page.viewport(width, height)?;
        let overflow = page.eval(
            "document.documentElement.scrollWidth <= innerWidth ? null : [document.documentElement.scrollWidth,
              [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth + 0.5)
                .map(e => e.className || e.tagName).slice(0, 6)]",
        )?;
        ensure!(overflow.is_null(), "horizontal scroll at {width}x{height}: {overflow}");
        let fits = page.eval(&format!(
            "[...document.querySelectorAll('.player button, .topbar button')].every(b => {{
                const r = b.getBoundingClientRect();
                return r.width >= 32 && r.height >= 32 && r.left >= 0 && r.right <= {width} && r.bottom <= {height};
            }})"
        ))?;
        ensure!(fits == Value::Bool(true), "controls do not fit at {width}x{height}");
        page.screenshot(&format!("fokus-light-{width}x{height}"))?;
    }
    page.viewport(1240, 800)?;
    let errors = page.errors()?;
    ensure!(errors.is_empty(), "page errors: {errors:?}");

    // Overlay bubble: transparent page, 56px hit circle, 34px face.
    for scheme in ["light", "dark"] {
        page.color_scheme(scheme)?;
        // A fresh document: changing only the hash would not reload the bundle.
        page.goto("about:blank")?;
        page.goto(&fixture.page_url("#/overlay"))?;
        page.wait("overlay bubble", "document.querySelector('.overlay-bubble.hidden')")?;
        let transparent = page.text("getComputedStyle(document.body).backgroundColor")?;
        ensure!(transparent == "rgba(0, 0, 0, 0)", "overlay background {transparent}");
        let sizes = page.eval(
            "(() => { const b = document.querySelector('.overlay-bubble').getBoundingClientRect();
                      const f = document.querySelector('.bubble-face').getBoundingClientRect();
                      return [b.width, b.height, f.width, f.height]; })()",
        )?;
        ensure!(sizes == serde_json::json!([56, 56, 34, 34]), "overlay sizes {sizes}");
        // Outside Tauri the bubble never becomes visible; show it for the picture
        // on a desktop-like background.
        page.eval(
            "document.querySelector('.overlay-bubble').classList.remove('hidden'),
             document.body.style.background = 'linear-gradient(135deg, #5b7083, #c9b79c)', true",
        )?;
        page.screenshot_clip(&format!("overlay-{scheme}"), 0.0, 0.0, 56.0, 56.0, 3.0)?;
    }
    let errors = page.errors()?;
    ensure!(errors.is_empty(), "overlay page errors: {errors:?}");

    println!(
        "PASS: settings/status, selection + char count, import, timed word highlight, pause freeze, \
         reading snapshot after reload, stop, import error, fallback notice, drawer (Escape/outside), \
         light/dark/override, speed readout, layouts 1240x800/1040x660/390x844, overlay bubble"
    );
    Ok(())
}
