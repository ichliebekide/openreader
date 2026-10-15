//! Port of `openreader_backend/services/document_service.py`.
//!
//! * PDF: pure Rust (`pdf-extract` on top of `lopdf`); Python used pypdf.
//! * EPUB: spine order from the OPF package, `nav` documents and
//!   `linear="no"` items skipped (like ebooklib).
//! * HTML: text without script/style/nav/head/hidden elements (like BeautifulSoup).
//! * OCR: the same external `tesseract` CLI as pytesseract, with the same
//!   image preparation (EXIF rotation, flatten on white, grayscale, 2x Lanczos
//!   for small screenshots).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use image::{DynamicImage, GrayImage, ImageDecoder, ImageReader, Luma};
use unicode_normalization::UnicodeNormalization;

use crate::text::normalize_selection;

const MAX_OCR_PIXELS: u64 = 40_000_000;

/// External OCR tool abstraction so tests do not need Tesseract.
pub trait OcrEngine: Send + Sync {
    /// Output of `tesseract --list-langs`.
    fn list_languages(&self) -> Result<String>;
    fn recognize(&self, image: &GrayImage, language: &str, timeout: Duration) -> Result<String>;
}

pub struct TesseractCli {
    pub command: PathBuf,
}

impl Default for TesseractCli {
    fn default() -> Self {
        Self {
            command: std::env::var_os("OPENREADER_TESSERACT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("tesseract")),
        }
    }
}

impl OcrEngine for TesseractCli {
    fn list_languages(&self) -> Result<String> {
        let output = run_with_timeout(
            Command::new(&self.command).arg("--list-langs"),
            None,
            Duration::from_secs(5),
        )
        .map_err(|_| anyhow!("Tesseract ist nicht verfuegbar oder antwortet nicht."))?;
        if !output.status.success() {
            bail!("Tesseract ist nicht verfuegbar oder antwortet nicht.");
        }
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        // Old Tesseract versions print the list to stderr.
        Ok(if stdout.trim().is_empty() {
            String::from_utf8_lossy(&output.stderr).into_owned()
        } else {
            stdout
        })
    }

    fn recognize(&self, image: &GrayImage, language: &str, timeout: Duration) -> Result<String> {
        let directory = tempfile::Builder::new().prefix("openreader-ocr-").tempdir()?;
        let input = directory.path().join("input.png");
        image.save(&input).context("OCR-Bild konnte nicht geschrieben werden")?;
        let output = run_with_timeout(
            Command::new(&self.command).arg(&input).arg("stdout").arg("-l").arg(language),
            None,
            timeout,
        )?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!("Tesseract ist fehlgeschlagen: {detail}");
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

pub struct DocumentService {
    pub ocr: Box<dyn OcrEngine>,
}

impl Default for DocumentService {
    fn default() -> Self {
        Self {
            ocr: Box::new(TesseractCli::default()),
        }
    }
}

impl DocumentService {
    pub fn extract_text(&self, path: &Path, language: &str) -> Result<String> {
        let suffix = path
            .extension()
            .map(|ext| format!(".{}", ext.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        match suffix.as_str() {
            ".pdf" => self.extract_pdf(path),
            ".epub" => extract_epub(path),
            ".png" | ".jpg" | ".jpeg" | ".tif" | ".tiff" | ".webp" => self.extract_ocr(path, language),
            ".txt" | ".md" => decode_text_file(&std::fs::read(path)?).map(|text| normalize_selection(&text)),
            ".html" | ".htm" | ".xhtml" => Ok(html_text(&std::fs::read(path)?)),
            _ => bail!("Unsupported document type: {suffix}"),
        }
    }

    fn extract_pdf(&self, path: &Path) -> Result<String> {
        let raw = pdf_raw_text(path)?;
        let text = normalize_selection(&raw);
        if text.is_empty() {
            bail!("PDF ohne Textschicht: Bitte die gescannten Seiten als Bilder importieren.");
        }
        Ok(text)
    }

    fn extract_ocr(&self, path: &Path, language: &str) -> Result<String> {
        let code = ocr_language(language)?;
        let languages = self.ocr.list_languages()?;
        let installed: Vec<&str> = languages.lines().skip(1).map(str::trim).collect();
        if !installed.contains(&code) {
            bail!("OCR-Sprachdaten fehlen: {code}. Installiere tesseract-ocr-{code}.");
        }
        let prepared = prepare_ocr_image(path)?;
        let text = self.ocr.recognize(&prepared, code, Duration::from_secs(30))?;
        Ok(normalize_selection(&text.nfc().collect::<String>()))
    }
}

pub fn decode_text_file(data: &[u8]) -> Result<String> {
    let invalid = || anyhow!("Die Textdatei muss UTF-8 oder UTF-16 mit BOM verwenden.");
    if data.starts_with(&[0xff, 0xfe]) || data.starts_with(&[0xfe, 0xff]) {
        let little = data[0] == 0xff;
        let body = &data[2..];
        if body.len() % 2 != 0 {
            return Err(invalid());
        }
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        return String::from_utf16(&units).map_err(|_| invalid());
    }
    let body = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    String::from_utf8(body.to_vec()).map_err(|_| invalid())
}

pub fn pdf_raw_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let result = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(error)) => bail!("PDF konnte nicht gelesen werden: {error}"),
        Err(_) => bail!("PDF konnte nicht gelesen werden."),
    }
}

// ---------------------------------------------------------------- HTML / EPUB

pub fn html_text(data: &[u8]) -> String {
    use scraper::Node;

    let source = String::from_utf8_lossy(data);
    let document = scraper::Html::parse_document(&source);
    let mut parts: Vec<String> = Vec::new();
    let mut stack = vec![document.tree.root()];
    while let Some(node) = stack.pop() {
        match node.value() {
            Node::Element(element) => {
                let name = element.name();
                let hidden = element.attr("hidden").is_some()
                    || element.attr("aria-hidden") == Some("true");
                if matches!(name, "script" | "style" | "nav" | "head") || hidden {
                    continue;
                }
            }
            Node::Text(text) => {
                parts.push(text.to_string());
                continue;
            }
            Node::Comment(_) | Node::Doctype(_) | Node::ProcessingInstruction(_) => continue,
            _ => {}
        }
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev());
    }
    normalize_selection(&parts.join(" "))
}

struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn attributes(element: &quick_xml::events::BytesStart<'_>) -> Vec<(Vec<u8>, String)> {
    element
        .attributes()
        .flatten()
        .map(|attribute| {
            let value = attribute
                .unescape_value()
                .map(|value| value.into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(&attribute.value).into_owned());
            (local_name(attribute.key.as_ref()).to_vec(), value)
        })
        .collect()
}

fn attribute(attributes: &[(Vec<u8>, String)], name: &str) -> Option<String> {
    attributes
        .iter()
        .find(|(key, _)| key == name.as_bytes())
        .map(|(_, value)| value.clone())
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn join_zip_path(base_dir: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(href));
    let mut parts: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').filter(|part| !part.is_empty()).collect()
    };
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

pub fn extract_epub(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).context("EPUB ist kein gueltiges ZIP-Archiv")?;
    let read_entry = |archive: &mut zip::ZipArchive<std::fs::File>, name: &str| -> Result<Vec<u8>> {
        let mut entry = archive
            .by_name(name)
            .with_context(|| format!("EPUB-Eintrag fehlt: {name}"))?;
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        Ok(data)
    };

    let container = read_entry(&mut archive, "META-INF/container.xml")?;
    let mut opf_path = None;
    let mut reader = quick_xml::Reader::from_reader(container.as_slice());
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element)
                if local_name(element.name().as_ref()) == b"rootfile" =>
            {
                opf_path = attribute(&attributes(&element), "full-path");
                if opf_path.is_some() {
                    break;
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let opf_path = opf_path.ok_or_else(|| anyhow!("EPUB ohne OPF-Paketdatei"))?;
    let base_dir = opf_path.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default();

    let opf = read_entry(&mut archive, &opf_path)?;
    let mut manifest: Vec<ManifestItem> = Vec::new();
    let mut spine: Vec<(String, String)> = Vec::new();
    let mut reader = quick_xml::Reader::from_reader(opf.as_slice());
    loop {
        match reader.read_event_into(&mut buffer)? {
            quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element) => {
                let attrs = attributes(&element);
                match local_name(element.name().as_ref()) {
                    b"item" => manifest.push(ManifestItem {
                        id: attribute(&attrs, "id").unwrap_or_default(),
                        href: attribute(&attrs, "href").unwrap_or_default(),
                        media_type: attribute(&attrs, "media-type").unwrap_or_default(),
                        properties: attribute(&attrs, "properties").unwrap_or_default(),
                    }),
                    b"itemref" => spine.push((
                        attribute(&attrs, "idref").unwrap_or_default(),
                        attribute(&attrs, "linear").unwrap_or_else(|| "yes".into()),
                    )),
                    _ => {}
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let is_document = |item: &ManifestItem| {
        matches!(item.media_type.as_str(), "application/xhtml+xml" | "text/html")
    };
    let items: Vec<&ManifestItem> = if spine.is_empty() {
        manifest.iter().filter(|item| is_document(item)).collect()
    } else {
        spine
            .iter()
            .filter(|(_, linear)| linear != "no")
            .filter_map(|(idref, _)| manifest.iter().find(|item| &item.id == idref))
            .collect()
    };

    let mut parts = Vec::new();
    for item in items {
        let is_nav = item.properties.split_whitespace().any(|property| property == "nav");
        if !is_document(item) || is_nav {
            continue;
        }
        let name = join_zip_path(&base_dir, &item.href);
        let Ok(data) = read_entry(&mut archive, &name) else {
            continue;
        };
        parts.push(html_text(&data));
    }
    Ok(normalize_selection(&parts.join("\n")))
}

// ----------------------------------------------------------------------- OCR

pub fn ocr_language(language: &str) -> Result<&'static str> {
    const ALIASES: [(&str, &str); 31] = [
        ("german", "deu"),
        ("deutsch", "deu"),
        ("de", "deu"),
        ("deu", "deu"),
        ("english", "eng"),
        ("en", "eng"),
        ("eng", "eng"),
        ("french", "fra"),
        ("fr", "fra"),
        ("fra", "fra"),
        ("spanish", "spa"),
        ("es", "spa"),
        ("spa", "spa"),
        ("italian", "ita"),
        ("it", "ita"),
        ("ita", "ita"),
        ("portuguese", "por"),
        ("pt", "por"),
        ("por", "por"),
        ("japanese", "jpn"),
        ("ja", "jpn"),
        ("jpn", "jpn"),
        ("korean", "kor"),
        ("ko", "kor"),
        ("kor", "kor"),
        ("chinese", "chi_sim"),
        ("zh", "chi_sim"),
        ("chi_sim", "chi_sim"),
        ("russian", "rus"),
        ("ru", "rus"),
        ("rus", "rus"),
    ];
    let lookup = |key: &str| {
        ALIASES
            .iter()
            .find(|(alias, _)| *alias == key)
            .map(|(_, code)| *code)
    };
    let lower = language.to_lowercase();
    let replaced = lower.replace('_', "-");
    let key = replaced.split('-').next().unwrap_or("");
    lookup(&lower)
        .or_else(|| lookup(key))
        .ok_or_else(|| anyhow!("Keine OCR-Sprachzuordnung fuer das Voice-Profil: {language}"))
}

pub fn prepare_ocr_image(path: &Path) -> Result<GrayImage> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if width as u64 * height as u64 > MAX_OCR_PIXELS {
        bail!("OCR-Bild zu gross (maximal 40 Megapixel). Bitte zuschneiden.");
    }
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Ok(flatten_and_scale(&image))
}

/// RGBA over white, PIL "L" luma (ITU-R 601-2), 2x Lanczos for small images.
pub fn flatten_and_scale(image: &DynamicImage) -> GrayImage {
    let rgba = image.to_rgba8();
    let mut gray = GrayImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        let alpha = a as u32;
        let blend = |channel: u8| (channel as u32 * alpha + 255 * (255 - alpha) + 127) / 255;
        let (r, g, b) = (blend(r), blend(g), blend(b));
        let luma = (r * 299 + g * 587 + b * 114 + 500) / 1000;
        gray.put_pixel(x, y, Luma([luma.min(255) as u8]));
    }
    let (width, height) = gray.dimensions();
    if width.max(height) < 2000 && width as u64 * height as u64 <= 3_000_000 {
        gray = image::imageops::resize(&gray, width * 2, height * 2, image::imageops::FilterType::Lanczos3);
    }
    gray
}

// ----------------------------------------------------------------- processes

/// Run a command with an optional stdin payload and a hard timeout (kills on expiry).
pub fn run_with_timeout(
    command: &mut Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<std::process::Output> {
    command
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(data)?;
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut data = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut data);
        }
        data
    });
    let err_thread = std::thread::spawn(move || {
        let mut data = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut data);
        }
        data
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Tesseract process timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(std::process::Output {
        status,
        stdout: out_thread.join().unwrap_or_default(),
        stderr: err_thread.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn unicode_text_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        let text = "Gr\u{fc}\u{df}e aus K\u{f6}ln!";
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend(text.encode_utf16().flat_map(|unit| unit.to_le_bytes()));
        let mut utf16be = vec![0xfe, 0xff];
        utf16be.extend(text.encode_utf16().flat_map(|unit| unit.to_be_bytes()));
        let mut sig = vec![0xef, 0xbb, 0xbf];
        sig.extend_from_slice(text.as_bytes());
        for bytes in [text.as_bytes().to_vec(), sig, utf16, utf16be] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(DocumentService::default().extract_text(&path, "German").unwrap(), text);
        }
    }

    #[test]
    fn invalid_encoding_is_not_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        std::fs::write(&path, b"Gr\xfc\xdfe").unwrap();
        let error = DocumentService::default().extract_text(&path, "German").unwrap_err();
        assert!(error.to_string().contains("UTF-8"));
    }

    #[test]
    fn html_omits_scripts_styles_and_navigation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.html");
        std::fs::write(
            &path,
            "<html><head><title>Metadata</title></head><body><nav>Menu</nav>\
             <p>Hallo Welt.</p><style>body{color:red}</style><script>danger()</script>\
             <p hidden>hidden</p><p aria-hidden=\"true\">hidden</p><p>Ende.</p></body></html>",
        )
        .unwrap();
        assert_eq!(
            DocumentService::default().extract_text(&path, "German").unwrap(),
            "Hallo Welt. Ende."
        );
    }

    fn write_epub(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default();
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("EPUB/content.opf", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">test</dc:identifier><dc:title>Testbuch</dc:title><dc:language>de</dc:language></metadata>
<manifest>
<item href="second.xhtml" id="second" media-type="application/xhtml+xml"/>
<item href="first.xhtml" id="first" media-type="application/xhtml+xml"/>
<item href="extra.xhtml" id="extra" media-type="application/xhtml+xml"/>
<item href="nav.xhtml" id="nav" media-type="application/xhtml+xml" properties="nav"/>
</manifest>
<spine><itemref idref="nav"/><itemref idref="first"/><itemref idref="extra" linear="no"/><itemref idref="second"/></spine>
</package>"#,
        )
        .unwrap();
        for (name, text) in [
            ("second", "Zweites Kapitel."),
            ("first", "Erstes Kapitel."),
            ("extra", "Nicht linear."),
            ("nav", "Inhaltsverzeichnis"),
        ] {
            zip.start_file(format!("EPUB/{name}.xhtml"), deflated).unwrap();
            zip.write_all(format!("<html><body><p>{text}</p></body></html>").as_bytes())
                .unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn epub_uses_spine_instead_of_manifest_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path);
        assert_eq!(
            DocumentService::default().extract_text(&path, "German").unwrap(),
            "Erstes Kapitel. Zweites Kapitel."
        );
    }

    /// Minimal valid PDF with correct xref offsets.
    fn write_pdf(path: &Path, content: &str) {
        let stream = content.as_bytes();
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            pdf.extend(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1)
                .as_bytes(),
        );
        std::fs::write(path, pdf).unwrap();
    }

    #[test]
    fn scan_only_pdf_explains_missing_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.pdf");
        write_pdf(&path, "");
        let error = DocumentService::default().extract_text(&path, "German").unwrap_err();
        assert!(error.to_string().contains("Textschicht"), "{error}");
    }

    #[test]
    fn pdf_text_layer_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.pdf");
        write_pdf(&path, "BT /F1 18 Tf 20 50 Td (Hallo PDF Welt.) Tj ET");
        assert_eq!(DocumentService::default().extract_text(&path, "German").unwrap(), "Hallo PDF Welt.");
    }

    /// LibreOffice PDF (embedded subset fonts with ToUnicode), German text in
    /// two balanced columns; source: testdata/zweispaltig.fodt.
    #[test]
    fn german_two_column_pdf_keeps_umlauts_and_column_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zweispaltig.pdf");
        std::fs::write(&path, include_bytes!("testdata/zweispaltig.pdf")).unwrap();
        let text = DocumentService::default().extract_text(&path, "German").unwrap();
        assert!(text.starts_with("Überschrift: Größere Übungen für Öffentlichkeit Grüße aus Köln!"), "{text}");
        for phrase in [
            "Jörg ärgert sich über die Verspätung des Zuges, während Käthe gemütlich Tee trinkt.",
            "Das Ergebnis lautet 12,50 € pro Quadratmeter.",
            "Anführungszeichen „so“ und Gedankenstriche – wie diesen.",
            "Ölkännchen, Ähre, Übel, Fuß, Maß und Spaß",
        ] {
            assert!(text.contains(phrase), "missing {phrase:?} in {text}");
        }
        // Column 1 is read completely before column 2.
        let first_column_end = text.find("Text sicher in die zweite Spalte umbricht.").unwrap();
        let second_column_start = text.find("Übermäßig lange Sätze").unwrap();
        assert!(first_column_end < second_column_start);
        assert!(text.ends_with("damit abgeschlossen."));
    }

    #[test]
    fn umlauts_in_simple_font_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("umlaut.pdf");
        // WinAnsiEncoding: \374 = ü, \337 = ß, \366 = ö.
        write_pdf(&path, "BT /F1 18 Tf 20 50 Td (Gr\\374\\337e aus K\\366ln) Tj ET");
        assert_eq!(DocumentService::default().extract_text(&path, "German").unwrap(), "Grüße aus Köln");
    }

    struct FakeOcr {
        languages: &'static str,
        calls: Mutex<Vec<(String, Duration)>>,
        result: Result<&'static str, &'static str>,
    }

    impl OcrEngine for FakeOcr {
        fn list_languages(&self) -> Result<String> {
            Ok(self.languages.to_string())
        }
        fn recognize(&self, image: &GrayImage, language: &str, timeout: Duration) -> Result<String> {
            self.calls.lock().unwrap().push((language.to_string(), timeout));
            assert_eq!(image.dimensions(), (200, 100));
            assert_eq!(image.get_pixel(0, 0).0[0], 255);
            match self.result {
                Ok(text) => Ok(text.to_string()),
                Err(error) => Err(anyhow!(error)),
            }
        }
    }

    fn service_with(ocr: FakeOcr) -> DocumentService {
        DocumentService {
            ocr: Box::new(ocr),
        }
    }

    #[test]
    fn ocr_selects_language_and_flattens_transparency() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        image::RgbaImage::from_pixel(100, 50, image::Rgba([0, 0, 0, 0])).save(&path).unwrap();
        let ocr = FakeOcr {
            languages: "List of languages (2):\ndeu\neng\n",
            calls: Mutex::new(Vec::new()),
            result: Ok("Gro\u{308}\u{df}e\nund Text."),
        };
        let service = service_with(ocr);
        assert_eq!(service.extract_text(&path, "de-DE").unwrap(), "Gr\u{f6}\u{df}e und Text.");
    }

    #[test]
    fn missing_german_ocr_does_not_silently_use_english() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_with(FakeOcr {
            languages: "List of languages (1):\neng\n",
            calls: Mutex::new(Vec::new()),
            result: Ok(""),
        });
        let error = service
            .extract_text(&dir.path().join("unused.png"), "German")
            .unwrap_err();
        assert!(error.to_string().contains("tesseract-ocr-deu"));
    }

    #[test]
    fn ocr_timeout_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        image::RgbImage::from_pixel(100, 50, image::Rgb([255, 255, 255])).save(&path).unwrap();
        let service = service_with(FakeOcr {
            languages: "List of languages (1):\neng\n",
            calls: Mutex::new(Vec::new()),
            result: Err("Tesseract process timeout"),
        });
        let error = service.extract_text(&path, "English").unwrap_err();
        assert!(error.to_string().contains("timeout"));
    }

    #[test]
    fn unknown_ocr_language_is_explicit() {
        assert!(ocr_language("unknown").unwrap_err().to_string().contains("Sprachzuordnung"));
        assert_eq!(ocr_language("pt_BR").unwrap(), "por");
        assert_eq!(ocr_language("chi_sim").unwrap(), "chi_sim");
    }

    #[test]
    fn real_tesseract_reads_prepared_image() {
        if std::env::var("OPENREADER_TEST_OCR").as_deref() != Ok("1") {
            return; // opt-in, requires tesseract-ocr-eng
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.png");
        // Render the text with ffmpeg's drawtext filter (ffmpeg is a runtime dependency anyway).
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=white:s=1400x160"])
            .args(["-frames:v", "1", "-vf"])
            .arg("drawtext=text='OpenReader reads selected text.':fontcolor=black:fontsize=40:x=30:y=60")
            .arg(&path)
            .status()
            .expect("ffmpeg");
        assert!(status.success());
        let text = DocumentService::default().extract_text(&path, "English").unwrap();
        assert_eq!(text, "OpenReader reads selected text.");
    }
}
