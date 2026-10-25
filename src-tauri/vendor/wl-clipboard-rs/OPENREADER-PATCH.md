# Vendored wl-clipboard-rs 0.9.4 (patched)

Unmodified copy of [wl-clipboard-rs](https://github.com/YaLTeR/wl-clipboard-rs)
0.9.4 (MIT OR Apache-2.0, see the licence files) with one change in
`src/watch.rs`: a data-control `offer` (MIME type) event for an offer that is
no longer tracked is ignored instead of hitting `unwrap()` on `None`.

That panic ended OpenReader's selection helper several times a day; the
selection made at that moment was lost. Remove this directory and the
`[patch.crates-io]` entry in `src-tauri/Cargo.toml` once upstream ships a fix.
