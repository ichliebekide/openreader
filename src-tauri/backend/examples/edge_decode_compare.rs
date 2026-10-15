//! Fetch one short Edge response and compare symphonia's PCM with ffmpeg's
//! (the decoder used before). Needs network and an `ffmpeg` binary.
//!
//! `cargo run -p openreader-backend --example edge_decode_compare`

use futures::StreamExt;
use openreader_backend::tts::edge::communicate::{communicate, EdgeMessage, TtsConfig};
use openreader_backend::tts::edge::mp3;

#[tokio::main]
async fn main() {
    let config = TtsConfig::new("de-DE-SeraphinaMultilingualNeural", "+0%", "+0%", "+0Hz", true).unwrap();
    let mut stream = Box::pin(communicate("Hallo Welt, das ist ein Vergleich der Dekoder.", config));
    let mut mp3_bytes = Vec::new();
    while let Some(message) = stream.next().await {
        if let EdgeMessage::Audio(data) = message.unwrap() {
            mp3_bytes.extend(data);
        }
    }

    let (input, mut pcm) = mp3::spawn_decoder();
    for chunk in mp3_bytes.chunks(4096) {
        input.send(chunk.to_vec()).unwrap();
    }
    drop(input);
    let mut ours: Vec<i16> = Vec::new();
    while let Some(block) = pcm.recv().await {
        ours.extend(block.unwrap().0);
    }

    let mut child = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i", "pipe:0", "-f", "s16le", "-acodec", "pcm_s16le"])
        .args(["-ar", "24000", "-ac", "1", "pipe:1"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&mp3_bytes).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let theirs: Vec<i16> = output.stdout.chunks_exact(2).map(|p| i16::from_le_bytes([p[0], p[1]])).collect();

    // Best alignment within +-2000 samples.
    let mut best = (i64::MAX, 0i64);
    for shift in -2000i64..=2000 {
        let mut error = 0i64;
        let mut count = 0;
        for (index, sample) in ours.iter().enumerate().step_by(7) {
            let other = index as i64 + shift;
            if other >= 0 && (other as usize) < theirs.len() {
                error += (*sample as i64 - theirs[other as usize] as i64).abs();
                count += 1;
            }
        }
        if count > 0 && error / count < best.0 {
            best = (error / count, shift);
        }
    }
    println!(
        "mp3 {} bytes | symphonia {} samples | ffmpeg {} samples | best shift {} samples, mean |diff| {}",
        mp3_bytes.len(),
        ours.len(),
        theirs.len(),
        best.1,
        best.0
    );
}
