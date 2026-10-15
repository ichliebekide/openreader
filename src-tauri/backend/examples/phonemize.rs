//! Print Piper phonemes (one JSON array of sentence strings per input line).
//!
//! `cargo run -p openreader-backend --example phonemize -- [voice] < text.txt`
//! Used to compare the Rust phonemizer with piper-tts on a corpus.

use std::io::BufRead;

fn main() {
    let voice = std::env::args().nth(1).unwrap_or_else(|| "de".into());
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("stdin");
        let sentences = openreader_backend::tts::piper::espeak::phonemize(&voice, &line).expect("phonemize");
        let strings: Vec<String> = sentences.iter().map(|sentence| sentence.iter().collect()).collect();
        println!("{}", serde_json::to_string(&strings).unwrap());
    }
}
