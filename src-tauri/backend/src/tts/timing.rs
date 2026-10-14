//! Port of `tts/timing.py`: map service words to the original text using
//! browser UTF-16 offsets.

use super::WordBoundary;

pub struct WordMatcher {
    text: String,
    /// Byte cursor into `text`.
    cursor: usize,
    utf16_cursor: i64,
}

fn utf16_len(text: &str) -> i64 {
    text.encode_utf16().count() as i64
}

impl WordMatcher {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cursor: 0,
            utf16_cursor: 0,
        }
    }

    /// `offset` and `duration` are in 100 ns ticks (as sent by the service).
    pub fn match_word(&mut self, word: &str, offset: i64, duration: i64) -> Option<WordBoundary> {
        if word.is_empty() {
            return None;
        }
        let start = self.cursor + self.text[self.cursor..].find(word)?;
        let end = start + word.len();
        let utf16_start = self.utf16_cursor + utf16_len(&self.text[self.cursor..start]);
        let utf16_end = utf16_start + utf16_len(word);
        self.cursor = end;
        self.utf16_cursor = utf16_end;
        Some(WordBoundary {
            start: utf16_start,
            end: utf16_end,
            offset: offset as f64 / 10_000_000.0,
            duration: duration as f64 / 10_000_000.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_matching_preserves_repeated_words_entities_and_utf16() {
        let mut matcher = WordMatcher::new("\u{1f600} Hallo, Hallo & Welt!");
        let words: Vec<_> = ["Hallo", "Hallo", "&", "Welt"]
            .iter()
            .enumerate()
            .map(|(i, word)| matcher.match_word(word, i as i64 * 10_000_000, 5_000_000).unwrap())
            .collect();
        let ranges: Vec<_> = words.iter().map(|word| (word.start, word.end)).collect();
        assert_eq!(ranges, [(3, 8), (10, 15), (16, 17), (18, 22)]);
        let offsets: Vec<_> = words.iter().map(|word| word.offset).collect();
        assert_eq!(offsets, [0.0, 1.0, 2.0, 3.0]);
        assert!(words.iter().all(|word| word.duration == 0.5));
        assert!(matcher.match_word("Hallo", 0, 1).is_none());
    }

    #[test]
    fn unmatched_words_are_not_given_invented_offsets() {
        let mut matcher = WordMatcher::new("Version 12 funktioniert.");
        assert!(matcher.match_word("zwoelf", 0, 1).is_none());
        assert!(matcher.match_word("", 0, 1).is_none());
        assert_eq!(matcher.match_word("funktioniert", 1, 1).unwrap().start, 11);
    }
}
