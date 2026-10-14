//! Native port of edge-tts 7.3.1 `Communicate.stream()`.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use futures::{SinkExt, Stream, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};

use super::drm;
use super::{SEC_MS_GEC_VERSION, WSS_URL};

pub const CHROMIUM_MAJOR_VERSION: &str = "143";
pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const MP3_BITRATE_BPS: i64 = 48_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub enum EdgeMessage {
    Audio(Vec<u8>),
    /// WordBoundary/SentenceBoundary metadata; offset/duration in 100 ns ticks.
    Boundary {
        kind: String,
        offset: i64,
        duration: i64,
        text: String,
    },
}

#[derive(Clone, Debug)]
pub struct TtsConfig {
    pub voice: String,
    pub rate: String,
    pub volume: String,
    pub pitch: String,
    pub word_boundary: bool,
}

impl TtsConfig {
    pub fn new(voice: &str, rate: &str, volume: &str, pitch: &str, word_boundary: bool) -> Result<Self> {
        let voice = expand_voice_name(voice);
        let valid_voice = voice.starts_with("Microsoft Server Speech Text to Speech Voice (")
            && voice.ends_with(')')
            && voice.contains(',');
        if !valid_voice {
            bail!("Invalid voice '{voice}'.");
        }
        let percent = |value: &str| {
            let rest = value.strip_prefix(['+', '-'])?.strip_suffix('%')?;
            (!rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit())).then_some(())
        };
        if percent(rate).is_none() {
            bail!("Invalid rate '{rate}'.");
        }
        if percent(volume).is_none() {
            bail!("Invalid volume '{volume}'.");
        }
        let pitch_ok = pitch
            .strip_prefix(['+', '-'])
            .and_then(|rest| rest.strip_suffix("Hz"))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()));
        if !pitch_ok {
            bail!("Invalid pitch '{pitch}'.");
        }
        Ok(Self {
            voice,
            rate: rate.into(),
            volume: volume.into(),
            pitch: pitch.into(),
            word_boundary,
        })
    }
}

/// `de-DE-SeraphinaMultilingualNeural` ->
/// `Microsoft Server Speech Text to Speech Voice (de-DE, SeraphinaMultilingualNeural)`.
pub fn expand_voice_name(voice: &str) -> String {
    // ^([a-z]{2,})-([A-Z]{2,})-(.+Neural)$
    let mut parts = voice.splitn(3, '-');
    let (Some(lang), Some(region), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return voice.to_string();
    };
    let lang_ok = lang.len() >= 2 && lang.chars().all(|ch| ch.is_ascii_lowercase());
    let region_ok = region.len() >= 2 && region.chars().all(|ch| ch.is_ascii_uppercase());
    let name_ok = name.len() > "Neural".len() && name.ends_with("Neural");
    if !(lang_ok && region_ok && name_ok) {
        return voice.to_string();
    }
    let (region, name) = match name.find('-') {
        Some(index) => (format!("{region}-{}", &name[..index]), &name[index + 1..]),
        None => (region.to_string(), name),
    };
    format!("Microsoft Server Speech Text to Speech Voice ({lang}-{region}, {name})")
}

/// `xml.sax.saxutils.escape`.
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('>', "&gt;").replace('<', "&lt;")
}

/// `xml.sax.saxutils.unescape`.
pub fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

pub fn remove_incompatible_characters(text: &str) -> String {
    text.chars()
        .map(|ch| match ch as u32 {
            0..=8 | 11..=12 | 14..=31 => ' ',
            _ => ch,
        })
        .collect()
}

fn strip_ascii_ws(bytes: &[u8]) -> &[u8] {
    // Python bytes.strip(): ASCII whitespace.
    let is_ws = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = bytes.iter().position(|byte| !is_ws(byte)).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|byte| !is_ws(byte)).map(|index| index + 1).unwrap_or(start);
    &bytes[start..end.max(start)]
}

fn rfind(haystack: &[u8], needle: u8, limit: usize) -> Option<usize> {
    haystack[..limit.min(haystack.len())].iter().rposition(|byte| *byte == needle)
}

fn safe_utf8_split_point(segment: &[u8]) -> usize {
    let mut split_at = segment.len();
    while split_at > 0 {
        if std::str::from_utf8(&segment[..split_at]).is_ok() {
            return split_at;
        }
        split_at -= 1;
    }
    split_at
}

fn adjust_for_xml_entity(text: &[u8], mut split_at: usize) -> usize {
    while split_at > 0 {
        let Some(ampersand) = text[..split_at].iter().rposition(|byte| *byte == b'&') else {
            break;
        };
        if text[ampersand..split_at].contains(&b';') {
            break;
        }
        split_at = ampersand;
    }
    split_at
}

/// `split_text_by_byte_length` from edge-tts (UTF-8 and XML entity safe).
pub fn split_text_by_byte_length(text: &str, byte_length: usize) -> Result<Vec<String>> {
    if byte_length == 0 {
        bail!("byte_length must be greater than 0");
    }
    let mut chunks = Vec::new();
    let mut text: &[u8] = text.as_bytes();
    while text.len() > byte_length {
        let split = rfind(text, b'\n', byte_length).or_else(|| rfind(text, b' ', byte_length));
        // Deviation from edge-tts 7.3.1: without whitespace, edge-tts looks for a
        // UTF-8 boundary in the *whole* remainder, which can exceed the limit.
        // Here the boundary is searched within `byte_length`.
        let split_at = match split {
            Some(index) => index,
            None => safe_utf8_split_point(&text[..byte_length]),
        };
        let split_at = adjust_for_xml_entity(text, split_at);
        let chunk = strip_ascii_ws(&text[..split_at]);
        if !chunk.is_empty() {
            chunks.push(String::from_utf8_lossy(chunk).into_owned());
        }
        text = &text[if split_at > 0 { split_at } else { 1 }..];
    }
    let remaining = strip_ascii_ws(text);
    if !remaining.is_empty() {
        chunks.push(String::from_utf8_lossy(remaining).into_owned());
    }
    Ok(chunks)
}

pub fn mkssml(config: &TtsConfig, escaped_text: &str) -> String {
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
         <voice name='{}'><prosody pitch='{}' rate='{}' volume='{}'>{}</prosody></voice></speak>",
        config.voice, config.pitch, config.rate, config.volume, escaped_text
    )
}

/// Javascript-style date string, always UTC (edge-tts does the same).
pub fn date_to_string() -> String {
    chrono::Utc::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

pub fn connect_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn ssml_headers_plus_data(request_id: &str, timestamp: &str, ssml: &str) -> String {
    format!(
        "X-RequestId:{request_id}\r\nContent-Type:application/ssml+xml\r\n\
         X-Timestamp:{timestamp}Z\r\nPath:ssml\r\n\r\n{ssml}"
    )
}

fn command_request(word_boundary: bool) -> String {
    let (wd, sq) = if word_boundary { ("true", "false") } else { ("false", "true") };
    format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\
         \"sentenceBoundaryEnabled\":\"{sq}\",\"wordBoundaryEnabled\":\"{wd}\"}},\
         \"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}\r\n",
        date_to_string()
    )
}

/// Split a service frame into header map and body (`get_headers_and_data`).
pub fn headers_and_data(data: &[u8], header_length: usize) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, &[u8])> {
    let header_length = header_length.min(data.len());
    let mut headers = Vec::new();
    for line in data[..header_length].split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or_else(|| anyhow!("Malformed header line from service"))?;
        headers.push((line[..colon].to_vec(), line[colon + 1..].to_vec()));
    }
    let body_start = (header_length + 2).min(data.len());
    Ok((headers, &data[body_start..]))
}

fn header<'a>(headers: &'a [(Vec<u8>, Vec<u8>)], name: &[u8]) -> Option<&'a [u8]> {
    headers
        .iter()
        .find(|(key, _)| key.as_slice() == name)
        .map(|(_, value)| value.as_slice())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

pub fn parse_metadata(data: &[u8], offset_compensation: i64) -> Result<Option<EdgeMessage>> {
    let value: serde_json::Value = serde_json::from_slice(data)?;
    let items = value["Metadata"]
        .as_array()
        .ok_or_else(|| anyhow!("Unexpected metadata from service"))?;
    for item in items {
        let kind = item["Type"].as_str().unwrap_or_default();
        match kind {
            "WordBoundary" | "SentenceBoundary" => {
                let data = &item["Data"];
                return Ok(Some(EdgeMessage::Boundary {
                    kind: kind.to_string(),
                    offset: data["Offset"].as_i64().unwrap_or(0) + offset_compensation,
                    duration: data["Duration"].as_i64().unwrap_or(0),
                    text: xml_unescape(data["text"]["Text"].as_str().unwrap_or_default()),
                }));
            }
            "SessionEnd" => continue,
            other => bail!("Unknown metadata type: {other}"),
        }
    }
    bail!("No WordBoundary metadata found")
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

enum ConnectError {
    Forbidden(Option<String>),
    Other(anyhow::Error),
}

async fn connect() -> Result<Socket, ConnectError> {
    let url = format!(
        "{WSS_URL}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={SEC_MS_GEC_VERSION}",
        connect_id(),
        drm::generate_sec_ms_gec()
    );
    let mut request = url
        .into_client_request()
        .map_err(|error| ConnectError::Other(error.into()))?;
    let user_agent = format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/{CHROMIUM_MAJOR_VERSION}.0.0.0 Safari/537.36 Edg/{CHROMIUM_MAJOR_VERSION}.0.0.0"
    );
    let cookie = format!("muid={};", drm::generate_muid());
    let headers = request.headers_mut();
    for (name, value) in [
        ("Pragma", "no-cache".to_string()),
        ("Cache-Control", "no-cache".to_string()),
        ("Origin", "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".to_string()),
        ("User-Agent", user_agent),
        ("Accept-Encoding", "gzip, deflate, br, zstd".to_string()),
        ("Accept-Language", "en-US,en;q=0.9".to_string()),
        ("Cookie", cookie),
    ] {
        headers.insert(name, HeaderValue::from_str(&value).expect("static header"));
    }
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request)).await {
        Err(_) => Err(ConnectError::Other(anyhow!("Microsoft-Sprachdienst: Verbindungs-Timeout"))),
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(tungstenite::Error::Http(response))) if response.status().as_u16() == 403 => {
            let date = response
                .headers()
                .get("Date")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            Err(ConnectError::Forbidden(date))
        }
        Ok(Err(tungstenite::Error::Http(response))) => Err(ConnectError::Other(anyhow!(
            "Microsoft-Sprachdienst antwortet mit HTTP {}",
            response.status()
        ))),
        Ok(Err(error)) => Err(ConnectError::Other(anyhow!("Microsoft-Sprachdienst nicht erreichbar: {error}"))),
    }
}

struct ChunkState {
    offset_compensation: i64,
    cumulative_audio_bytes: i64,
}

/// Stream one SSML request (`__stream` in edge-tts).
fn stream_part(
    socket: Socket,
    config: TtsConfig,
    partial_text: String,
    state: std::sync::Arc<std::sync::Mutex<ChunkState>>,
) -> impl Stream<Item = Result<EdgeMessage>> {
    async_stream::try_stream! {
        let mut socket = socket;
        socket.send(Message::Text(command_request(config.word_boundary).into())).await?;
        socket
            .send(Message::Text(
                ssml_headers_plus_data(&connect_id(), &date_to_string(), &mkssml(&config, &partial_text)).into(),
            ))
            .await?;

        let mut audio_was_received = false;
        let mut chunk_audio_bytes: i64 = 0;
        loop {
            let message = match tokio::time::timeout(RECEIVE_TIMEOUT, socket.next()).await {
                Err(_) => Err(anyhow!("Microsoft-Sprachdienst: Empfangs-Timeout"))?,
                Ok(None) => break,
                Ok(Some(message)) => message?,
            };
            match message {
                Message::Text(text) => {
                    let data = text.as_bytes();
                    let header_end = find(data, b"\r\n\r\n").unwrap_or(data.len());
                    let (headers, body) = headers_and_data(data, header_end)?;
                    match header(&headers, b"Path") {
                        Some(b"audio.metadata") => {
                            let compensation = state.lock().unwrap().offset_compensation;
                            if let Some(message) = parse_metadata(body, compensation)? {
                                yield message;
                            }
                        }
                        Some(b"turn.end") => {
                            let mut state = state.lock().unwrap();
                            state.cumulative_audio_bytes += chunk_audio_bytes;
                            state.offset_compensation =
                                state.cumulative_audio_bytes * 8 * TICKS_PER_SECOND / MP3_BITRATE_BPS;
                            break;
                        }
                        Some(b"response") | Some(b"turn.start") => {}
                        _ => Err(anyhow!("Unknown path received"))?,
                    }
                }
                Message::Binary(data) => {
                    if data.len() < 2 {
                        Err(anyhow!("We received a binary message, but it is missing the header length."))?;
                    }
                    let header_length = u16::from_be_bytes([data[0], data[1]]) as usize;
                    if header_length > data.len() {
                        Err(anyhow!("The header length is greater than the length of the data."))?;
                    }
                    let (headers, body) = headers_and_data(&data, header_length)?;
                    if header(&headers, b"Path") != Some(b"audio".as_slice()) {
                        Err(anyhow!("Received binary message, but the path is not audio."))?;
                    }
                    match header(&headers, b"Content-Type") {
                        None if body.is_empty() => continue,
                        None => Err(anyhow!("Received binary message with no Content-Type, but with data."))?,
                        Some(b"audio/mpeg") => {}
                        Some(_) => Err(anyhow!("Received binary message, but with an unexpected Content-Type."))?,
                    }
                    if body.is_empty() {
                        Err(anyhow!("Received binary message, but it is missing the audio data."))?;
                    }
                    audio_was_received = true;
                    chunk_audio_bytes += body.len() as i64;
                    yield EdgeMessage::Audio(body.to_vec());
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = socket.close(None).await;
        if !audio_was_received {
            Err(anyhow!("No audio was received. Please verify that your parameters are correct."))?;
        }
    }
}

/// Stream audio and boundary metadata for `text` (any length; split at 4096
/// bytes of escaped text, one websocket per part, like edge-tts).
pub fn communicate(text: &str, config: TtsConfig) -> impl Stream<Item = Result<EdgeMessage>> + Send + 'static {
    let texts = split_text_by_byte_length(&xml_escape(&remove_incompatible_characters(text)), 4096);
    async_stream::try_stream! {
        let texts = texts?;
        let state = std::sync::Arc::new(std::sync::Mutex::new(ChunkState {
            offset_compensation: 0,
            cumulative_audio_bytes: 0,
        }));
        for partial in texts {
            let socket = match connect().await {
                Ok(socket) => socket,
                Err(ConnectError::Forbidden(date)) => {
                    // Clock skew: adjust from the server date and retry once.
                    let date = date.ok_or_else(|| anyhow!("No server date in headers."))?;
                    drm::adjust_clock_skew(&date).map_err(|error| anyhow!(error))?;
                    match connect().await {
                        Ok(socket) => socket,
                        Err(ConnectError::Forbidden(_)) => Err(anyhow!(
                            "Microsoft-Sprachdienst lehnt die Anfrage ab (HTTP 403, Sec-MS-GEC)"
                        ))?,
                        Err(ConnectError::Other(error)) => Err(error)?,
                    }
                }
                Err(ConnectError::Other(error)) => Err(error)?,
            };
            let mut part = Box::pin(stream_part(socket, config.clone(), partial, state.clone()));
            while let Some(message) = part.next().await {
                yield message?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_names_are_expanded_like_edge_tts() {
        assert_eq!(
            expand_voice_name("de-DE-SeraphinaMultilingualNeural"),
            "Microsoft Server Speech Text to Speech Voice (de-DE, SeraphinaMultilingualNeural)"
        );
        assert_eq!(
            expand_voice_name("zh-CN-liaoning-XiaobeiNeural"),
            "Microsoft Server Speech Text to Speech Voice (zh-CN-liaoning, XiaobeiNeural)"
        );
        assert_eq!(expand_voice_name("custom"), "custom");
        assert!(TtsConfig::new("custom", "+0%", "+0%", "+0Hz", true).is_err());
        assert!(TtsConfig::new("de-DE-SeraphinaMultilingualNeural", "+0", "+0%", "+0Hz", true).is_err());
        assert!(TtsConfig::new("de-DE-SeraphinaMultilingualNeural", "-25%", "+100%", "+0Hz", true).is_ok());
    }

    #[test]
    fn byte_split_matches_edge_tts() {
        // Reference: edge_tts.communicate.split_text_by_byte_length(escape('a & b <c> ' * 3), 12)
        let parts = split_text_by_byte_length(&xml_escape(&"a & b <c> ".repeat(3)), 12).unwrap();
        assert_eq!(parts, ["a &amp; b", "&lt;c&gt;", "a &amp; b", "&lt;c&gt;", "a &amp; b", "&lt;c&gt;"]);
        let unicode = split_text_by_byte_length(&"ä".repeat(10), 5).unwrap();
        assert_eq!(unicode, ["ää", "ää", "ää", "ää", "ää"]);
        assert!(split_text_by_byte_length("x", 0).is_err());
    }

    #[test]
    fn incompatible_characters_become_spaces() {
        assert_eq!(remove_incompatible_characters("a\u{b}b\u{c}c\nd\re\u{1}"), "a b c\nd\re ");
    }

    #[test]
    fn metadata_and_frames_are_parsed() {
        let frame = b"X-RequestId:abc\r\nContent-Type:application/json\r\nPath:audio.metadata\r\n\r\n\
{\"Metadata\":[{\"Type\":\"WordBoundary\",\"Data\":{\"Offset\":1000000,\"Duration\":3000000,\"text\":{\"Text\":\"R&amp;D\",\"Length\":3,\"BoundaryType\":\"WordBoundary\"}}}]}";
        let end = find(frame, b"\r\n\r\n").unwrap();
        let (headers, body) = headers_and_data(frame, end).unwrap();
        assert_eq!(header(&headers, b"Path"), Some(b"audio.metadata".as_slice()));
        let message = parse_metadata(body, 50).unwrap().unwrap();
        assert_eq!(
            message,
            EdgeMessage::Boundary {
                kind: "WordBoundary".into(),
                offset: 1_000_050,
                duration: 3_000_000,
                text: "R&D".into()
            }
        );
        assert!(parse_metadata(br#"{"Metadata":[{"Type":"Viseme"}]}"#, 0).is_err());
    }

    #[test]
    fn ssml_matches_edge_tts() {
        let config = TtsConfig::new("de-DE-SeraphinaMultilingualNeural", "+25%", "-10%", "+0Hz", true).unwrap();
        assert_eq!(
            mkssml(&config, "Hallo &amp; Welt"),
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
             <voice name='Microsoft Server Speech Text to Speech Voice (de-DE, SeraphinaMultilingualNeural)'>\
             <prosody pitch='+0Hz' rate='+25%' volume='-10%'>Hallo &amp; Welt</prosody></voice></speak>"
        );
        assert!(command_request(true).contains("\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"true\"},\"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}\r\n"));
    }
}
