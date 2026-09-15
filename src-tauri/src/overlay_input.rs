use gtk::{
    cairo::{RectangleInt, Region},
    prelude::*,
};
use tauri::WebviewWindow;

pub fn configure(window: &WebviewWindow) {
    if let Ok(native) = window.gtk_window() {
        // GTK can discard a pre-realization resize on a non-resizable window.
        native.set_default_size(56, 56);
        native.set_size_request(56, 56);
        native.resize(56, 56);
        native.input_shape_combine_region(Some(&Region::create()));
    }
}

// CSS and the configured window both use a 56 logical-pixel circle.
fn bubble_region() -> Region {
    let mut rows = Vec::new();
    for y in 0..56 {
        let dy = y as f64 + 0.5 - 28.0;
        let half_width = (28.0_f64.powi(2) - dy * dy).sqrt();
        let left = (28.0 - half_width - 0.5).ceil() as i32;
        let right = (28.0 + half_width - 0.5).floor() as i32;
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
        for y in -1..=56 {
            for x in -1..=56 {
                let dx = x as f64 + 0.5 - 28.0;
                let dy = y as f64 + 0.5 - 28.0;
                assert_eq!(
                    region.contains_point(x, y),
                    dx * dx + dy * dy <= 28.0 * 28.0
                );
            }
        }
    }
}
