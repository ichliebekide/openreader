//! Update check: asks GitHub for the newest OpenReader release once a day
//! (if enabled in the settings) and reports it when it is newer than this
//! build. Nothing is downloaded or installed; the UI only links to the
//! release page. The request carries no user data, only the app version in
//! the User-Agent.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;

use crate::models::{ReleaseInfo, UpdateInfo};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/ichliebekide/openreader/releases/latest";
const RELEASE_PAGE_PREFIX: &str = "https://github.com/ichliebekide/openreader/releases/";
const NOTES_CHARS: usize = 600;
/// First check shortly after start, then once a day.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// `Ok(None)` when the repository has no release yet.
pub type Fetch = fn() -> Result<Option<Value>>;

pub struct UpdateChecker {
    current: String,
    fetch: Fetch,
    info: Mutex<UpdateInfo>,
}

impl UpdateChecker {
    pub fn new(current: &str) -> Arc<Self> {
        Self::with_fetch(current, fetch_latest_release)
    }

    pub fn with_fetch(current: &str, fetch: Fetch) -> Arc<Self> {
        Arc::new(Self {
            current: current.to_string(),
            fetch,
            info: Mutex::new(UpdateInfo {
                current: current.to_string(),
                ..UpdateInfo::default()
            }),
        })
    }

    /// Result of the last check (not checked yet: no `checked_at`).
    pub fn info(&self) -> UpdateInfo {
        self.info.lock().unwrap().clone()
    }

    /// Ask GitHub now and remember the result.
    pub async fn check(self: &Arc<Self>) -> UpdateInfo {
        let checker = self.clone();
        let result = tokio::task::spawn_blocking(move || (checker.fetch)())
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
        let mut info = self.info.lock().unwrap();
        info.checked_at = Some(Utc::now());
        match result.and_then(|release| match release {
            Some(release) => newer_release(&release, &self.current),
            None => Ok(None),
        }) {
            Ok(available) => {
                if let Some(release) = &available {
                    tracing::info!("update available: OpenReader {}", release.version);
                }
                info.available = available;
                info.error = None;
            }
            Err(error) => {
                tracing::info!("update check failed: {error:#}");
                info.error = Some(format!("{error:#}"));
            }
        }
        info.clone()
    }
}

fn fetch_latest_release() -> Result<Option<Value>> {
    let response = ureq::get(LATEST_RELEASE_API)
        .timeout(Duration::from_secs(10))
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("OpenReader/{}", crate::VERSION))
        .call();
    match response {
        Ok(response) => {
            let body = response.into_string().context("GitHub-Antwort unlesbar")?;
            Ok(Some(serde_json::from_str(&body).context("GitHub-Antwort unlesbar")?))
        }
        // No release published yet.
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(ureq::Error::Status(code, _)) => anyhow::bail!("GitHub antwortet mit HTTP {code}"),
        Err(error) => Err(anyhow::anyhow!("GitHub nicht erreichbar: {error}")),
    }
}

/// The release from a GitHub `releases/latest` response if it is newer than
/// `current`; drafts and pre-releases are ignored.
pub fn newer_release(release: &Value, current: &str) -> Result<Option<ReleaseInfo>> {
    if release["draft"].as_bool() == Some(true) || release["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = release["tag_name"].as_str().context("Release ohne tag_name")?;
    let (Some(latest), Some(running)) = (parse_version(tag), parse_version(current)) else {
        anyhow::bail!("Versionsnummer nicht lesbar: {tag}");
    };
    if latest <= running {
        return Ok(None);
    }
    // Only ever link to this project's release pages.
    let url = release["html_url"]
        .as_str()
        .filter(|url| url.starts_with(RELEASE_PAGE_PREFIX))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{RELEASE_PAGE_PREFIX}latest"));
    let body = release["body"].as_str().unwrap_or_default();
    let title = release["name"].as_str().unwrap_or_default();
    let security = [title, body].iter().any(|text| {
        let text = text.to_lowercase();
        text.contains("security") || text.contains("sicherheit")
    });
    let mut notes: String = body.trim().chars().take(NOTES_CHARS).collect();
    if body.trim().chars().count() > NOTES_CHARS {
        notes.push('…');
    }
    Ok(Some(ReleaseInfo {
        version: format!("{}.{}.{}", latest.0, latest.1, latest.2),
        url,
        notes,
        security,
    }))
}

/// `v0.1.2`, `0.1.2` (a pre-release suffix like `-rc.1` is cut off).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next().unwrap_or(Some(0))?, parts.next().unwrap_or(Some(0))?);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, body: &str) -> Value {
        json!({
            "tag_name": tag,
            "name": format!("OpenReader {tag}"),
            "html_url": format!("{RELEASE_PAGE_PREFIX}tag/{tag}"),
            "body": body,
            "draft": false,
            "prerelease": false,
        })
    }

    #[test]
    fn versions_are_compared_numerically() {
        assert_eq!(parse_version("v0.1.10"), Some((0, 1, 10)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("v2.0.0-rc.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(parse_version("0.1.10") > parse_version("0.1.9"));
    }

    #[test]
    fn only_newer_stable_releases_are_offered() {
        let newer = newer_release(&release("v0.1.1", "Behebt eine Sicherheitsluecke."), "0.1.0").unwrap().unwrap();
        assert_eq!(newer.version, "0.1.1");
        assert_eq!(newer.url, format!("{RELEASE_PAGE_PREFIX}tag/v0.1.1"));
        assert!(newer.security);
        assert!(newer_release(&release("v0.1.0", ""), "0.1.0").unwrap().is_none());
        assert!(newer_release(&release("v0.0.9", ""), "0.1.0").unwrap().is_none());
        let mut pre = release("v0.2.0", "");
        pre["prerelease"] = json!(true);
        assert!(newer_release(&pre, "0.1.0").unwrap().is_none());
    }

    #[test]
    fn foreign_links_are_not_trusted_and_notes_are_short() {
        let mut odd = release("v0.2.0", &"x".repeat(2000));
        odd["html_url"] = json!("https://evil.example/download");
        let info = newer_release(&odd, "0.1.0").unwrap().unwrap();
        assert_eq!(info.url, format!("{RELEASE_PAGE_PREFIX}latest"));
        assert_eq!(info.notes.chars().count(), NOTES_CHARS + 1);
        assert!(!info.security);
    }

    #[tokio::test]
    async fn check_remembers_results_and_errors() {
        fn newer() -> Result<Option<Value>> {
            Ok(Some(release("v9.0.0", "Neu")))
        }
        fn none() -> Result<Option<Value>> {
            Ok(None)
        }
        fn offline() -> Result<Option<Value>> {
            anyhow::bail!("GitHub nicht erreichbar")
        }
        let checker = UpdateChecker::with_fetch("0.1.0", newer);
        assert!(checker.info().checked_at.is_none());
        let info = checker.check().await;
        assert_eq!(info.available.unwrap().version, "9.0.0");
        assert!(checker.info().checked_at.is_some());

        let checker = UpdateChecker::with_fetch("0.1.0", none);
        assert_eq!(checker.check().await.available, None);

        let checker = UpdateChecker::with_fetch("0.1.0", offline);
        let info = checker.check().await;
        assert_eq!(info.error.as_deref(), Some("GitHub nicht erreichbar"));
        assert_eq!(info.current, "0.1.0");
    }

    /// Against the real GitHub API (opt-in).
    #[tokio::test]
    async fn real_github_check() {
        if std::env::var("OPENREADER_TEST_UPDATE").as_deref() != Ok("1") {
            return;
        }
        let info = UpdateChecker::new("0.0.1").check().await;
        eprintln!("update check: {info:?}");
        assert!(info.error.is_none(), "{:?}", info.error);
    }
}
