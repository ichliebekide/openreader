use gtk::{
    cairo::{RectangleInt, Region},
    prelude::*,
};
use std::sync::atomic::{AtomicI32, Ordering};

use tauri::WebviewWindow;

/// Logical size of the overlay window and its circular input region (button
/// face + margin, see `ReaderSettings::overlay_bubble_px`); matches the CSS.
static SIZE: AtomicI32 = AtomicI32::new(72);

fn size() -> i32 {
    SIZE.load(Ordering::Relaxed)
}

pub fn configure(window: &WebviewWindow) {
    if let Ok(native) = window.gtk_window() {
        apply_size(&native);
        native.input_shape_combine_region(Some(&Region::create()));
    }
}

fn apply_size(native: &gtk::ApplicationWindow) {
    let size = size();
    // GTK can discard a pre-realization resize on a non-resizable window.
    native.set_default_size(size, size);
    native.set_size_request(size, size);
    native.resize(size, size);
}

/// Resize the overlay window and its input circle. Runs on the main thread
/// (synchronous Tauri commands do) before the window is shown.
pub fn resize(window: &WebviewWindow, new_size: i32) {
    if SIZE.swap(new_size, Ordering::Relaxed) == new_size {
        return;
    }
    let logical = tauri::LogicalSize::new(new_size as f64, new_size as f64);
    let _ = window.set_min_size(Some(logical));
    let _ = window.set_max_size(Some(logical));
    let _ = window.set_size(logical);
    if let Ok(native) = window.gtk_window() {
        apply_size(&native);
    }
}

// CSS and the configured window both use a SIZE logical-pixel circle.
fn bubble_region() -> Region {
    let size = size();
    let r = size as f64 / 2.0;
    let mut rows = Vec::new();
    for y in 0..size {
        let dy = y as f64 + 0.5 - r;
        let half_width = (r.powi(2) - dy * dy).sqrt();
        let left = (r - half_width - 0.5).ceil() as i32;
        let right = (r + half_width - 0.5).floor() as i32;
        rows.push(RectangleInt::new(left, y, right - left + 1, 1));
    }
    Region::create_rectangles(&rows)
}

pub fn set_enabled(window: WebviewWindow, enabled: bool) -> Result<(), String> {
    let dispatcher = window.clone();
    dispatcher
        .run_on_main_thread(move || {
            if let Ok(native) = window.gtk_window() {
                // A delayed animation callback must not reactivate a hidden window.
                let region = if enabled && native.is_visible() {
                    bubble_region()
                } else {
                    Region::create()
                };
                native.input_shape_combine_region(Some(&region));
            }
        })
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_matches_circle_and_leaves_transparent_corners_clear() {
        let region = bubble_region();
        let size = size();
        let r = size as f64 / 2.0;
        for y in -1..=size {
            for x in -1..=size {
                let dx = x as f64 + 0.5 - r;
                let dy = y as f64 + 0.5 - r;
                assert_eq!(region.contains_point(x, y), dx * dx + dy * dy <= r * r);
            }
        }
    }
}
