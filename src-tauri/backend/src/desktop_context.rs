//! Port of `openreader_backend/services/desktop_context.py`.
//!
//! The optional KWin script calls `org.openreader.Desktop.ReportContext` on
//! the session bus with the cursor position and the active window class.

use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::models::DesktopContextSnapshot;

#[derive(Clone, Default)]
pub struct DesktopContext {
    snapshot: Arc<Mutex<DesktopContextSnapshot>>,
}

impl DesktopContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, cursor_x: i32, cursor_y: i32, resource_class: &str, resource_name: &str) {
        let non_empty = |value: &str| (!value.is_empty()).then(|| value.to_string());
        *self.snapshot.lock().unwrap() = DesktopContextSnapshot {
            cursor_x: Some(cursor_x),
            cursor_y: Some(cursor_y),
            active_resource_class: non_empty(resource_class),
            active_resource_name: non_empty(resource_name),
            updated_at: Utc::now(),
        };
    }

    pub fn snapshot(&self) -> DesktopContextSnapshot {
        self.snapshot.lock().unwrap().clone()
    }
}

struct DesktopInterface {
    context: DesktopContext,
}

#[zbus::interface(name = "org.openreader.Desktop")]
impl DesktopInterface {
    #[zbus(name = "ReportContext")]
    fn report_context(&self, cursor_x: i32, cursor_y: i32, resource_class: String, resource_name: String) -> bool {
        self.context.update(cursor_x, cursor_y, &resource_class, &resource_name);
        true
    }
}

/// Serve the bridge until the session bus goes away. Errors are logged and
/// swallowed, like the Python `suppress(Exception)`.
pub async fn run_dbus_bridge(context: DesktopContext) {
    let connection = async {
        zbus::connection::Builder::session()?
            .name("org.openreader.Desktop")?
            .serve_at("/org/openreader/Desktop", DesktopInterface { context })?
            .build()
            .await
    }
    .await;
    match connection {
        Ok(connection) => {
            tracing::info!("D-Bus desktop context bridge registered as org.openreader.Desktop");
            // Keep the connection alive; zbus dispatches calls on its own executor task.
            let _connection = connection;
            std::future::pending::<()>().await;
        }
        Err(error) => tracing::info!("D-Bus desktop context bridge unavailable: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_names_become_none() {
        let context = DesktopContext::new();
        assert_eq!(context.snapshot().cursor_x, None);
        context.update(10, 20, "konsole", "");
        let snapshot = context.snapshot();
        assert_eq!((snapshot.cursor_x, snapshot.cursor_y), (Some(10), Some(20)));
        assert_eq!(snapshot.active_resource_class.as_deref(), Some("konsole"));
        assert_eq!(snapshot.active_resource_name, None);
    }
}
