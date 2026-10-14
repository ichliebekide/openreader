//! Port of `openreader_backend/utils/text.py`.
//!
//! Lengths are counted in Unicode scalar values like Python's `len(str)`.

use std::sync::OnceLock;

use regex::Regex;

/// Python's `str.isspace()` (used by `\s` in `re` str patterns) also matches
/// the ASCII information separators U+001C..U+001F.
pub fn is_python_space(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

/// Python `str.isalnum() or "_"`, i.e. `\w` in a str pattern.
pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if is_python_space(ch) {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

pub fn normalize_selection(text: &str) -> String {
    let clean = decode_unicode_escapes(&text.replace('\0', ""));
    let clean = htmlize::unescape(clean.as_str()).into_owned();
    collapse_whitespace(&clean)
}

fn unicode_escape_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\\u([0-9a-fA-F]{4})|\\U([0-9a-fA-F]{8})").unwrap())
}

pub fn decode_unicode_escapes(text: &str) -> String {
    if !text.contains("\\u") && !text.contains("\\U") {
        return text.to_string();
    }
    unicode_escape_re()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let value = captures.get(1).or_else(|| captures.get(2)).unwrap().as_str();
            u32::from_str_radix(value, 16)
                .ok()
                .and_then(char::from_u32)
                .map(String::from)
                // Python would produce a lone surrogate; keep the escape instead.
                .unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}

pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

pub fn preview(text: &str, max_chars: usize) -> String {
    let clean = normalize_selection(text);
    if char_len(&clean) <= max_chars {
        return clean;
    }
    let mut short: String = clean.chars().take(max_chars.saturating_sub(1)).collect();
    short.push('…');
    short
}

pub const DEFAULT_PREVIEW_CHARS: usize = 520;

#[derive(Debug, PartialEq, Eq)]
pub struct ChunkSizeError;

impl std::fmt::Display for ChunkSizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("max_chars must be positive")
    }
}

impl std::error::Error for ChunkSizeError {}

const SENTENCE_END: [char; 6] = ['.', '!', '?', '。', '！', '？'];
const CLOSERS: [char; 7] = ['"', '\u{201c}', '\u{201d}', '\u{bb}', '\u{2019}', '\'', ')'];

fn is_closer(ch: char) -> bool {
    CLOSERS.contains(&ch) || ch == ']'
}

/// Byte offsets where Python's
/// `(?<=[.!?。！？])(?:["“”»’')\]]*)\s+` matches end (non-overlapping, left to right).
fn sentence_boundary_ends(text: &str) -> Vec<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ends = Vec::new();
    let mut index = 1;
    while index < chars.len() {
        if !SENTENCE_END.contains(&chars[index - 1].1) {
            index += 1;
            continue;
        }
        let mut cursor = index;
        while cursor < chars.len() && is_closer(chars[cursor].1) {
            cursor += 1;
        }
        let space_start = cursor;
        while cursor < chars.len() && is_python_space(chars[cursor].1) {
            cursor += 1;
        }
        if cursor > space_start {
            let end = chars.get(cursor).map(|(offset, _)| *offset).unwrap_or(text.len());
            ends.push(end);
            index = cursor.max(index + 1);
        } else {
            index += 1;
        }
    }
    ends
}

pub fn split_sentences(text: &str, max_chars: usize) -> Result<Vec<String>, ChunkSizeError> {
    if max_chars < 1 {
        return Err(ChunkSizeError);
    }
    let clean = normalize_selection(text);
    if clean.is_empty() {
        return Ok(Vec::new());
    }

    // Slice the original text instead of dropping punctuation or splitting decimals.
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0;
    for end in sentence_boundary_ends(&clean) {
        parts.push(clean[start..end].trim_matches(is_python_space));
        start = end;
    }
    parts.push(&clean[start..]);

    let mut sentences = Vec::new();
    let mut buffer = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let part_len = char_len(part);
        if part_len > max_chars {
            if !buffer.is_empty() {
                sentences.push(std::mem::take(&mut buffer));
            }
            sentences.extend(split_long_text(part, max_chars)?);
            continue;
        }
        if char_len(&buffer) + part_len + 1 <= max_chars {
            buffer = format!("{buffer} {part}").trim_matches(is_python_space).to_string();
            continue;
        }
        if !buffer.is_empty() {
            sentences.push(std::mem::take(&mut buffer));
        }
        buffer = part.to_string();
    }
    if !buffer.is_empty() {
        sentences.push(buffer);
    }
    if sentences.is_empty() {
        sentences.push(clean);
    }
    Ok(sentences)
}

pub fn split_long_text(text: &str, max_chars: usize) -> Result<Vec<String>, ChunkSizeError> {
    if max_chars < 1 {
        return Err(ChunkSizeError);
    }
    let mut chunks = Vec::new();
    let mut buffer = String::new();
    for word in text.split(' ') {
        if word.is_empty() {
            continue;
        }
        let chars: Vec<char> = word.chars().collect();
        if chars.len() > max_chars {
            if !buffer.is_empty() {
                chunks.push(std::mem::take(&mut buffer));
            }
            chunks.extend(chars.chunks(max_chars).map(|piece| piece.iter().collect::<String>()));
            continue;
        }
        let candidate = format!("{buffer} {word}").trim_matches(is_python_space).to_string();
        if char_len(&candidate) <= max_chars {
            buffer = candidate;
            continue;
        }
        if !buffer.is_empty() {
            chunks.push(std::mem::take(&mut buffer));
        }
        buffer = word.to_string();
    }
    if !buffer.is_empty() {
        chunks.push(buffer);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: [&str; 6] = [
        "Das kostet 3.14 Euro. Version 1.2.3 bleibt gleich!",
        "https://example.org/path?q=1.2 ist die URL. Danach geht es weiter.",
        "Wirklich?! Ja... Und dann: \"Fertig!\" Na gut.",
        "Herr Dr. Meier liest z. B. diesen Text. Ende.",
        "Hallo!!! Was??? ...",
        "你好！！ 下一句。 再见！",
    ];

    #[test]
    fn chunks_preserve_all_text() {
        for text in SAMPLES {
            for limit in [30, 60, 420] {
                let chunks = split_sentences(text, limit).unwrap();
                assert_eq!(chunks.join(" "), normalize_selection(text), "{text} / {limit}");
                assert!(chunks.iter().all(|chunk| {
                    let len = char_len(chunk);
                    len > 0 && len <= limit
                }));
            }
        }
    }

    #[test]
    fn sentence_boundaries_match_python_regex() {
        // Values produced by the Python implementation.
        assert_eq!(
            split_sentences("Erster Satz. Zweiter Satz! Dritter?", 14).unwrap(),
            ["Erster Satz.", "Zweiter Satz!", "Dritter?"]
        );
        assert_eq!(
            split_sentences("Er sagte: \"Ja!\" Dann ging er.", 16).unwrap(),
            ["Er sagte: \"Ja!\"", "Dann ging er."]
        );
    }

    #[test]
    fn long_unbroken_word_is_not_dropped() {
        let text = "x".repeat(1000);
        assert_eq!(split_sentences(&text, 90).unwrap().concat(), text);
    }

    #[test]
    fn invalid_chunk_size_fails_clearly() {
        assert_eq!(split_sentences("Text", 0).unwrap_err().to_string(), "max_chars must be positive");
        assert_eq!(split_long_text("Text", 0).unwrap_err().to_string(), "max_chars must be positive");
    }

    #[test]
    fn empty_text_has_no_chunks() {
        assert!(split_sentences(" \n ", 420).unwrap().is_empty());
    }

    #[test]
    fn normalization_matches_python() {
        assert_eq!(normalize_selection("  a\u{0}b \n\t c  "), "ab c");
        assert_eq!(normalize_selection("Gr\\u00fc\\u00dfe &amp; &lt;3 &auml;"), "Grüße & <3 ä");
        assert_eq!(normalize_selection("x\\U0001F600y"), "x\u{1F600}y");
        assert_eq!(normalize_selection("bad \\ud800 escape"), "bad \\ud800 escape");
        assert_eq!(normalize_selection("a\u{1c}b\u{a0}c"), "a b c");
    }

    #[test]
    fn preview_truncates_with_ellipsis() {
        assert_eq!(preview("abcdef", 4), "abc…");
        assert_eq!(preview("abc", 4), "abc");
        assert_eq!(preview("äöüß", 3), "äö…");
    }
}
