//! Word timings for Piper. The voices output audio only, so the duration of
//! each synthesized sentence is spread over its words, weighted by word length
//! with a little extra room for the pause after punctuation. The estimate is
//! re-anchored at every sentence, so it cannot drift across a long text.

use crate::tts::WordBoundary;

/// A word of the request text: UTF-16 offsets (like the engines report them)
/// and its relative speaking weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordSpan {
    pub start: i64,
    pub end: i64,
    weight: f64,
}

/// Whitespace-separated words of `line`, which starts at UTF-16 offset
/// `line_start` of the request text.
pub fn words_in(line: &str, line_start: i64) -> Vec<WordSpan> {
    let mut words = Vec::new();
    let mut position = line_start;
    let mut current: Option<(i64, String)> = None;
    for ch in line.chars() {
        let units = ch.len_utf16() as i64;
        if ch.is_whitespace() {
            if let Some((start, word)) = current.take() {
                words.push(span(start, position, &word));
            }
        } else {
            current.get_or_insert_with(|| (position, String::new())).1.push(ch);
        }
        position += units;
    }
    if let Some((start, word)) = current {
        words.push(span(start, position, &word));
    }
    words
}

fn span(start: i64, end: i64, word: &str) -> WordSpan {
    let letters = word.chars().filter(|ch| ch.is_alphanumeric()).count() as f64;
    let pause = match word.chars().last() {
        Some('.' | '!' | '?' | ':' | ';') => 4.0,
        Some(',' | ')' | '"' | '“' | '”' | '–' | '-') => 2.0,
        _ => 0.0,
    };
    WordSpan {
        start,
        end,
        // A word costs at least a syllable, plus the gap to the next one.
        weight: letters.max(2.0) + 1.0 + pause,
    }
}

/// Split `words` across sentences in proportion to how many spoken words
/// (space-separated phoneme groups) each sentence has. Pronunciation rules
/// and numbers can make the counts differ; the proportional split keeps the
/// mapping monotonic and complete anyway.
pub fn allocate<'a>(words: &'a [WordSpan], spoken_words: &[usize]) -> Vec<&'a [WordSpan]> {
    let total: usize = spoken_words.iter().sum();
    let mut parts = Vec::with_capacity(spoken_words.len());
    let mut taken = 0;
    let mut cumulative = 0;
    for (index, count) in spoken_words.iter().enumerate() {
        cumulative += count;
        let until = if index + 1 == spoken_words.len() {
            words.len()
        } else if total == 0 {
            words.len() * (index + 1) / spoken_words.len()
        } else {
            ((words.len() * cumulative) as f64 / total as f64).round() as usize
        }
        .clamp(taken, words.len());
        parts.push(&words[taken..until]);
        taken = until;
    }
    parts
}

/// Number of spoken words in a phonemized sentence (espeak separates words
/// with spaces).
pub fn spoken_words(phonemes: &[char]) -> usize {
    phonemes
        .split(|ch| *ch == ' ')
        .filter(|group| group.iter().any(|ch| ch.is_alphanumeric() || !ch.is_ascii()))
        .count()
}

/// Timings for `words` spoken in the audio from `start` to `start + duration`
/// seconds (relative to the start of the request's audio).
pub fn spread(words: &[WordSpan], start: f64, duration: f64) -> Vec<WordBoundary> {
    let total: f64 = words.iter().map(|word| word.weight).sum();
    if words.is_empty() || total <= 0.0 || duration <= 0.0 {
        return Vec::new();
    }
    let mut offset = start;
    words
        .iter()
        .map(|word| {
            let slot = duration * word.weight / total;
            let boundary = WordBoundary {
                start: word.start,
                end: word.end,
                offset,
                // Leave the highlight off during the gap after the word.
                duration: slot * 0.85,
            };
            offset += slot;
            boundary
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_use_utf16_offsets_of_the_request_text() {
        let words = words_in("Grüße 😀 Welt.", 10);
        let ranges: Vec<_> = words.iter().map(|word| (word.start, word.end)).collect();
        assert_eq!(ranges, [(10, 15), (16, 18), (19, 24)]);
    }

    #[test]
    fn words_are_shared_out_in_order() {
        let words = words_in("eins zwei drei vier fünf sechs", 0);
        let parts = allocate(&words, &[2, 4]);
        assert_eq!(parts[0].len(), 2);
        assert_eq!(parts[1].len(), 4);
        // More spoken words than written ones (e.g. a number read out).
        let parts = allocate(&words[..2], &[5, 1, 0]);
        assert_eq!(parts.iter().map(|part| part.len()).sum::<usize>(), 2);
        assert!(parts.last().unwrap().is_empty());
    }

    #[test]
    fn spoken_words_are_counted_from_phoneme_groups() {
        let phonemes: Vec<char> = "hˈaloː vˈɛlt , ˈdas".chars().collect();
        assert_eq!(spoken_words(&phonemes), 3);
    }

    #[test]
    fn timings_fill_the_sentence_in_order() {
        let words = words_in("Ein kurzer Satz.", 0);
        let timings = spread(&words, 2.0, 1.5);
        assert_eq!(timings.len(), 3);
        assert_eq!(timings[0].offset, 2.0);
        assert!(timings.windows(2).all(|pair| pair[0].offset + pair[0].duration <= pair[1].offset));
        let last = timings.last().unwrap();
        assert!(last.offset + last.duration <= 3.5 + 1e-9);
        assert!(timings[1].duration > timings[0].duration, "longer words take longer");
    }
}
