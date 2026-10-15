//! Data models mirroring `openreader_backend/models.py` (pydantic) field by field.
//!
//! The API types live in the shared, wasm-compatible `openreader-api` crate
//! (`src-tauri/api-types`), which the Leptos UI uses as well. This module
//! re-exports them and adds the backend-only models.

use chrono::{DateTime, Utc};
use serde::Serialize;

pub use openreader_api::*;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DesktopContextSnapshot {
    pub cursor_x: Option<i32>,
    pub cursor_y: Option<i32>,
    pub active_resource_class: Option<String>,
    pub active_resource_name: Option<String>,
    #[serde(serialize_with = "serialize_datetime")]
    pub updated_at: DateTime<Utc>,
}

impl Default for DesktopContextSnapshot {
    fn default() -> Self {
        Self {
            cursor_x: None,
            cursor_y: None,
            active_resource_class: None,
            active_resource_name: None,
            updated_at: Utc::now(),
        }
    }
}
