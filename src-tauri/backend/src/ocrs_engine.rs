//! Optional pure-Rust OCR with [ocrs](https://github.com/robertknight/ocrs).
//!
//! The default ocrs models recognize Latin letters, digits and ASCII
//! punctuation only (no umlauts or ß), so Tesseract stays the default for
//! German. Models (`text-detection.rten`, `text-recognition.rten`, about
//! 12 MB) are downloaded on first use into `~/.cache/openreader/ocrs/`, or
//! read from `$OPENREADER_OCRS_MODELS` (a directory with both files).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use image::GrayImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};

use crate::documents::ImageOcr;

pub const MODEL_BASE_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";

pub fn model_dir() -> PathBuf {
    std::env::var_os("OPENREADER_OCRS_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::cache_home().join("openreader/ocrs"))
}

#[derive(Default)]
pub struct OcrsEngine;

fn engine() -> Result<&'static Mutex<OcrEngine>> {
    static ENGINE: OnceLock<Result<Mutex<OcrEngine>, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| load().map(Mutex::new).map_err(|error| format!("{error:#}")))
        .as_ref()
        .map_err(|error| anyhow!("ocrs ist nicht verfuegbar: {error}"))
}

fn load() -> Result<OcrEngine> {
    let dir = model_dir();
    let mut models = Vec::new();
    for name in [DETECTION_MODEL, RECOGNITION_MODEL] {
        let path = dir.join(name);
        if std::env::var_os("OPENREADER_OCRS_MODELS").is_none() {
            crate::download::ensure_file(&format!("{MODEL_BASE_URL}/{name}"), &path)?;
        }
        models.push(rten::Model::load_file(&path).with_context(|| format!("ocrs-Modell {}", path.display()))?);
    }
    let recognition = models.pop();
    let detection = models.pop();
    OcrEngine::new(OcrEngineParams {
        detection_model: detection,
        recognition_model: recognition,
        ..Default::default()
    })
}

impl ImageOcr for OcrsEngine {
    fn recognize(&self, image: &GrayImage) -> Result<String> {
        let engine = engine()?.lock().unwrap();
        let source = ImageSource::from_bytes(image.as_raw(), image.dimensions())
            .map_err(|error| anyhow!("OCR-Bild: {error:?}"))?;
        let input = engine.prepare_input(source)?;
        engine.get_text(&input)
    }
}
