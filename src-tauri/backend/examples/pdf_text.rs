//! Print the text the document import extracts from a PDF (or other file).
//!
//! `cargo run -p openreader-backend --example pdf_text -- file.pdf [--raw]`

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).expect("file"));
    if std::env::args().any(|argument| argument == "--raw") {
        println!("{}", openreader_backend::documents::pdf_raw_text(&path).unwrap());
        return;
    }
    let service = openreader_backend::documents::DocumentService::default();
    match service.extract_text(&path, "German") {
        Ok(text) => println!("{text}"),
        Err(error) => println!("ERROR: {error:#}"),
    }
}
