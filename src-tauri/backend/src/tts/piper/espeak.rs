//! espeak-ng phonemization exactly like piper-tts 1.4.2 (`espeakbridge.c` +
//! `phonemize_espeak.py`).
//!
//! espeak-ng is built from source and linked statically (see `build.rs`); its
//! compiled data is embedded and unpacked to the cache on first use, so there
//! is no runtime dependency on a system espeak-ng.
//! Override: `OPENREADER_ESPEAK_DATA` (the directory that contains a full
//! `espeak-ng-data`, e.g. for more languages than the embedded de/en).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, bail, Result};
use unicode_normalization::UnicodeNormalization;

const AUDIO_OUTPUT_SYNCHRONOUS: c_int = 2;
const ESPEAK_CHARS_AUTO: c_int = 0;
const ESPEAK_PHONEMES_IPA: c_int = 0x02;

// espeak-ng translate.h clause flags.
const CLAUSE_INTONATION_FULL_STOP: c_int = 0x0000_0000;
const CLAUSE_INTONATION_COMMA: c_int = 0x0000_1000;
const CLAUSE_INTONATION_QUESTION: c_int = 0x0000_2000;
const CLAUSE_INTONATION_EXCLAMATION: c_int = 0x0000_3000;
const CLAUSE_TYPE_CLAUSE: c_int = 0x0004_0000;
const CLAUSE_TYPE_SENTENCE: c_int = 0x0008_0000;
const CLAUSE_PERIOD: c_int = 40 | CLAUSE_INTONATION_FULL_STOP | CLAUSE_TYPE_SENTENCE;
const CLAUSE_COMMA: c_int = 20 | CLAUSE_INTONATION_COMMA | CLAUSE_TYPE_CLAUSE;
const CLAUSE_QUESTION: c_int = 40 | CLAUSE_INTONATION_QUESTION | CLAUSE_TYPE_SENTENCE;
const CLAUSE_EXCLAMATION: c_int = 45 | CLAUSE_INTONATION_EXCLAMATION | CLAUSE_TYPE_SENTENCE;
const CLAUSE_COLON: c_int = 30 | CLAUSE_INTONATION_FULL_STOP | CLAUSE_TYPE_CLAUSE;
const CLAUSE_SEMICOLON: c_int = 30 | CLAUSE_INTONATION_COMMA | CLAUSE_TYPE_CLAUSE;

// Statically linked espeak-ng (built by build.rs from the commit piper-tts uses).
extern "C" {
    fn espeak_Initialize(output: c_int, buflength: c_int, path: *const c_char, options: c_int) -> c_int;
    fn espeak_SetVoiceByName(name: *const c_char) -> c_int;
    fn espeak_TextToPhonemesWithTerminator(
        textptr: *mut *const c_void,
        textmode: c_int,
        phonememode: c_int,
        terminator: *mut c_int,
    ) -> *const c_char;
}

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/espeak_data.rs"));
}

pub use embedded::ESPEAK_COMMIT;

struct Espeak {
    voice: Option<String>,
}

static ESPEAK: OnceLock<Mutex<Result<Espeak, String>>> = OnceLock::new();

/// Directory that contains `espeak-ng-data`: `$OPENREADER_ESPEAK_DATA`, or the
/// embedded data unpacked to `~/.cache/openreader/espeak-ng/<commit>-<n>/`.
pub fn data_parent() -> Result<std::path::PathBuf, String> {
    if let Some(dir) = std::env::var_os("OPENREADER_ESPEAK_DATA") {
        return Ok(dir.into());
    }
    let total: usize = embedded::ESPEAK_DATA.iter().map(|(_, data)| data.len()).sum();
    let parent = crate::cache_home()
        .join("openreader/espeak-ng")
        .join(format!("{}-{total}", &ESPEAK_COMMIT[..12]));
    let marker = parent.join(".complete");
    if !marker.exists() {
        let staging = parent.with_extension("partial");
        let _ = std::fs::remove_dir_all(&staging);
        for (relative, data) in embedded::ESPEAK_DATA {
            let path = staging.join("espeak-ng-data").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
            std::fs::write(&path, data).map_err(|error| error.to_string())?;
        }
        std::fs::write(staging.join(".complete"), ESPEAK_COMMIT).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::rename(&staging, &parent).map_err(|error| error.to_string())?;
    }
    Ok(parent)
}

fn load() -> Result<Espeak, String> {
    let parent = data_parent()?;
    let path = CString::new(parent.to_string_lossy().into_owned()).map_err(|error| error.to_string())?;
    if unsafe { espeak_Initialize(AUDIO_OUTPUT_SYNCHRONOUS, 0, path.as_ptr(), 0) } < 0 {
        return Err(format!("espeak-ng konnte nicht initialisiert werden ({})", parent.display()));
    }
    Ok(Espeak { voice: None })
}

/// One espeak clause: (phonemes, terminator, end of sentence) like `espeakbridge.get_phonemes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    pub phonemes: String,
    pub terminator: &'static str,
    pub end_of_sentence: bool,
}

fn categorize(terminator: c_int) -> (&'static str, bool) {
    match terminator & 0x000F_FFFF {
        CLAUSE_PERIOD => (".", true),
        CLAUSE_QUESTION => ("?", true),
        CLAUSE_EXCLAMATION => ("!", true),
        CLAUSE_COMMA => (",", false),
        CLAUSE_COLON => (":", false),
        CLAUSE_SEMICOLON => (";", false),
        _ => ("", false),
    }
}

/// Raw espeak clauses for `text` with voice `voice` (e.g. `de`).
pub fn clauses(voice: &str, text: &str) -> Result<Vec<Clause>> {
    let mutex = ESPEAK.get_or_init(|| Mutex::new(load()));
    let mut guard = mutex.lock().unwrap();
    let espeak = guard.as_mut().map_err(|error| anyhow!(error.clone()))?;
    if espeak.voice.as_deref() != Some(voice) {
        let name = CString::new(voice)?;
        if unsafe { espeak_SetVoiceByName(name.as_ptr()) } != 0 {
            bail!("espeak-ng kennt die Stimme {voice} nicht");
        }
        espeak.voice = Some(voice.to_string());
    }
    let text = CString::new(text.replace('\0', ""))?;
    let mut pointer: *const c_void = text.as_ptr().cast();
    let mut clauses = Vec::new();
    while !pointer.is_null() {
        let mut terminator: c_int = 0;
        let phonemes = unsafe {
            espeak_TextToPhonemesWithTerminator(&mut pointer, ESPEAK_CHARS_AUTO, ESPEAK_PHONEMES_IPA, &mut terminator)
        };
        let phonemes = if phonemes.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(phonemes) }.to_string_lossy().into_owned()
        };
        let (terminator, end_of_sentence) = categorize(terminator);
        clauses.push(Clause {
            phonemes,
            terminator,
            end_of_sentence,
        });
    }
    Ok(clauses)
}

/// Remove `(lang)` switch flags around foreign words (`\([^)]+\)`).
fn strip_language_flags(phonemes: &str) -> String {
    let mut out = String::with_capacity(phonemes.len());
    let mut rest = phonemes;
    while let Some(start) = rest.find('(') {
        match rest[start + 1..].find(')') {
            Some(length) if length > 0 => {
                out.push_str(&rest[..start]);
                rest = &rest[start + 1 + length + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `EspeakPhonemizer.phonemize`: phonemes (NFD code points) grouped by sentence.
pub fn group_sentences(clauses: &[Clause]) -> Vec<Vec<char>> {
    let mut sentences = Vec::new();
    let mut sentence: Vec<char> = Vec::new();
    for clause in clauses {
        let mut phonemes = strip_language_flags(&clause.phonemes);
        // Keep punctuation even though it's not technically a phoneme.
        phonemes.push_str(clause.terminator);
        if matches!(clause.terminator, "," | ":" | ";") {
            phonemes.push(' ');
        }
        sentence.extend(phonemes.nfd());
        if clause.end_of_sentence {
            sentences.push(std::mem::take(&mut sentence));
        }
    }
    if !sentence.is_empty() {
        sentences.push(sentence);
    }
    sentences
}

pub fn phonemize(voice: &str, text: &str) -> Result<Vec<Vec<char>>> {
    Ok(group_sentences(&clauses(voice, text)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_flags_are_removed() {
        assert_eq!(strip_language_flags("das ˈoːpən (en)ɹˈiːdə(de)"), "das ˈoːpən ɹˈiːdə");
        assert_eq!(strip_language_flags("a () b (c"), "a () b (c");
    }

    #[test]
    fn clauses_are_grouped_like_piper() {
        let clauses = [
            Clause { phonemes: "hˈaloː".into(), terminator: ",", end_of_sentence: false },
            Clause { phonemes: "vˈɛlt".into(), terminator: ".", end_of_sentence: true },
            Clause { phonemes: "nɑːx".into(), terminator: "", end_of_sentence: false },
        ];
        let sentences = group_sentences(&clauses);
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0].iter().collect::<String>(), "hˈaloː, vˈɛlt.");
        assert_eq!(sentences[1].iter().collect::<String>(), "nɑːx");
        // NFD splits precomposed characters such as ç into base + combining mark.
        let decomposed = group_sentences(&[Clause { phonemes: "ç".into(), terminator: "", end_of_sentence: false }]);
        assert_eq!(decomposed[0], vec!['c', '\u{327}']);
    }

    #[test]
    fn terminators_match_espeakbridge() {
        assert_eq!(categorize(CLAUSE_PERIOD), (".", true));
        assert_eq!(categorize(CLAUSE_QUESTION | 0x0010_0000), ("?", true));
        assert_eq!(categorize(CLAUSE_COMMA), (",", false));
        assert_eq!(categorize(40 | CLAUSE_TYPE_SENTENCE | 0x0001_0000), ("", false)); // EOF
    }
}
