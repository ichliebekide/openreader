//! Port of `openreader_backend/utils/pronunciation.py`.

use std::path::{Path, PathBuf};

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use crate::text::is_word_char;

pub const DEFAULT_REPLACEMENTS: [(&str, &str); 17] = [
    ("AI-TTS", "A I T T S"),
    ("AI", "A I"),
    ("API", "A P I"),
    ("CUDA", "Kuda"),
    ("EPUB", "E Pub"),
    ("FastAPI", "Fast A P I"),
    ("GPU", "G P U"),
    ("KDE", "K D E"),
    ("KWin", "K Win"),
    ("OCR", "O C R"),
    ("OpenReader", "Open Rieder"),
    ("PDF", "P D F"),
    ("Qwen", "Kwen"),
    ("TTS", "T T S"),
    ("Tauri", "Tau ri"),
    ("URL", "U R L"),
    ("Wayland", "Wehland"),
];

pub fn pronunciation_config_path() -> PathBuf {
    crate::config_home().join("openreader").join("pronunciations.json")
}

pub fn apply_pronunciations(text: &str) -> String {
    apply_with(text, &load_replacements(&pronunciation_config_path()))
}

pub fn apply_with(text: &str, replacements: &[(String, String)]) -> String {
    let mut ordered: Vec<&(String, String)> = replacements.iter().collect();
    // Stable sort, longest source first, exactly like Python's sorted(..., reverse=True)
    // on a key (reverse keeps the original order of equal keys).
    ordered.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));

    let mut normalized = text.to_string();
    for (source, target) in ordered {
        if source.is_empty() || target.is_empty() {
            continue;
        }
        normalized = replace_whole_word(&normalized, source, target);
    }
    normalized
}

/// `re.sub(rf"(?<!\w){re.escape(source)}(?!\w)", target, text)`.
fn replace_whole_word(text: &str, source: &str, target: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    let mut copied = 0;
    while let Some(found) = text[position..].find(source) {
        let start = position + found;
        let end = start + source.len();
        let before_ok = text[..start].chars().next_back().is_none_or(|ch| !is_word_char(ch));
        let after_ok = text[end..].chars().next().is_none_or(|ch| !is_word_char(ch));
        if before_ok && after_ok {
            out.push_str(&text[copied..start]);
            out.push_str(target);
            copied = end;
            position = end;
        } else {
            position = start + text[start..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
        if position >= text.len() {
            break;
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn defaults() -> Vec<(String, String)> {
    DEFAULT_REPLACEMENTS
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// Same text as `json.dumps(DEFAULT_REPLACEMENTS, indent=2, ensure_ascii=False)`.
pub fn default_file_contents() -> String {
    let lines: Vec<String> = DEFAULT_REPLACEMENTS
        .iter()
        .map(|(key, value)| {
            format!(
                "  {}: {}",
                serde_json::to_string(key).unwrap(),
                serde_json::to_string(value).unwrap()
            )
        })
        .collect();
    format!("{{\n{}\n}}", lines.join(",\n"))
}

pub fn load_replacements(path: &Path) -> Vec<(String, String)> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, default_file_contents());
        return defaults();
    }

    let Ok(text) = std::fs::read_to_string(path) else {
        return defaults();
    };
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    let Ok(OrderedObject(entries)) = OrderedObject::deserialize(&mut deserializer) else {
        return defaults();
    };
    if deserializer.end().is_err() {
        return defaults();
    }
    let Some(entries) = entries else {
        // Valid JSON, but not an object.
        return defaults();
    };

    let mut replacements = defaults();
    for (key, value) in entries {
        let value = match value {
            Value::Null => continue,
            Value::String(value) => value,
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            other => other.to_string(),
        };
        match replacements.iter_mut().find(|(existing, _)| *existing == key) {
            Some(entry) => entry.1 = value,
            None => replacements.push((key, value)),
        }
    }
    replacements
}

/// A JSON value that keeps object key order (a non-object yields `None`).
struct OrderedObject(Option<Vec<(String, Value)>>);

impl<'de> serde::Deserialize<'de> for OrderedObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = OrderedObject;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, Value)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    // Python dicts keep the first position of a duplicate key, with the last value.
                    match entries.iter_mut().find(|(existing, _)| *existing == key) {
                        Some(entry) => entry.1 = value,
                        None => entries.push((key, value)),
                    }
                }
                Ok(OrderedObject(Some(entries)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(OrderedObject(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(OrderedObject(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(OrderedObject(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(OrderedObject(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(OrderedObject(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(OrderedObject(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedObject(None))
            }
        }
        deserializer.deserialize_any(OrderedVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_whole_words_longest_first() {
        let rules = defaults();
        assert_eq!(apply_with("AI-TTS mit KDE", &rules), "A I T T S mit K D E");
        assert_eq!(apply_with("OpenReader liest PDFs und PDF.", &rules), "Open Rieder liest PDFs und P D F.");
        assert_eq!(apply_with("TTS_TTS xTTS TTS", &rules), "TTS_TTS xTTS T T S");
        assert_eq!(apply_with("ÄTTS TTSä TTS", &rules), "ÄTTS TTSä T T S");
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/pronunciations.json");
        assert_eq!(load_replacements(&path), defaults());
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("{\n  \"AI-TTS\": \"A I T T S\",\n  \"AI\": \"A I\","));
        let parsed: serde_json::Map<String, Value> = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed.len(), DEFAULT_REPLACEMENTS.len());
    }

    #[test]
    fn user_rules_extend_and_override_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pronunciations.json");
        std::fs::write(&path, r#"{"KDE": "Ka De E", "Rust": "Rast", "Nix": null}"#).unwrap();
        let rules = load_replacements(&path);
        assert_eq!(apply_with("KDE und Rust und Nix", &rules), "Ka De E und Rast und Nix");
        std::fs::write(&path, "[1, 2]").unwrap();
        assert_eq!(load_replacements(&path), defaults());
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(load_replacements(&path), defaults());
    }
}
