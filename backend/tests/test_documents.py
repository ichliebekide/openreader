from types import SimpleNamespace

import pytest
from ebooklib import epub
from PIL import Image
from pypdf import PdfWriter

from openreader_backend.services.document_service import DocumentService


@pytest.mark.parametrize("encoding", ["utf-8", "utf-8-sig", "utf-16"])
def test_unicode_text_import(tmp_path, encoding):
    path = tmp_path / "text.txt"
    path.write_bytes("Gr\u00fc\u00dfe aus K\u00f6ln!".encode(encoding))
    assert DocumentService().extract_text(path) == "Gr\u00fc\u00dfe aus K\u00f6ln!"


def test_invalid_encoding_is_not_silently_dropped(tmp_path):
    path = tmp_path / "text.txt"
    path.write_bytes(b"Gr\xfc\xdfe")
    with pytest.raises(ValueError, match="UTF-8"):
        DocumentService().extract_text(path)


def test_html_omits_scripts_styles_and_navigation(tmp_path):
    path = tmp_path / "page.html"
    path.write_text(
        "<html><head><title>Metadata</title></head><body><nav>Menu</nav>"
        "<p>Hallo Welt.</p><style>body{color:red}</style><script>danger()</script>"
        '<p hidden>hidden</p><p aria-hidden="true">hidden</p><p>Ende.</p></body></html>'
    )
    assert DocumentService().extract_text(path) == "Hallo Welt. Ende."


def test_epub_uses_spine_instead_of_manifest_order(tmp_path):
    book = epub.EpubBook()
    book.set_identifier("test")
    book.set_title("Testbuch")
    book.set_language("de")
    chapters = []
    for uid, text in [
        ("second", "Zweites Kapitel."),
        ("first", "Erstes Kapitel."),
        ("extra", "Nicht linear."),
    ]:
        chapter = epub.EpubHtml(uid=uid, file_name=f"{uid}.xhtml", title=uid)
        chapter.content = f"<html><body><p>{text}</p></body></html>"
        book.add_item(chapter)
        chapters.append(chapter)
    book.add_item(epub.EpubNav())
    book.spine = ["nav", "first", (chapters[2], "no"), "second"]
    path = tmp_path / "book.epub"
    epub.write_epub(str(path), book)
    assert DocumentService().extract_text(path) == "Erstes Kapitel. Zweites Kapitel."


def test_scan_only_pdf_explains_missing_text(tmp_path):
    path = tmp_path / "scan.pdf"
    writer = PdfWriter()
    writer.add_blank_page(width=100, height=100)
    writer.write(path)
    with pytest.raises(ValueError, match="Textschicht"):
        DocumentService().extract_text(path)


def test_ocr_selects_language_and_flattens_transparency(tmp_path, monkeypatch):
    from openreader_backend.services import document_service as module

    path = tmp_path / "capture.png"
    Image.new("RGBA", (100, 50), (0, 0, 0, 0)).save(path)
    monkeypatch.setattr(
        module.subprocess,
        "run",
        lambda *a, **k: SimpleNamespace(stdout="List of languages (2):\ndeu\neng\n"),
    )
    calls = []

    def recognize(image, **kwargs):
        calls.append(kwargs)
        assert image.mode == "L"
        assert image.size == (200, 100)
        assert image.getpixel((0, 0)) == 255
        return "Gro\u0308\u00dfe\nund Text."

    monkeypatch.setattr(module.pytesseract, "image_to_string", recognize)
    assert DocumentService().extract_text(path, "de-DE") == "Gr\u00f6\u00dfe und Text."
    assert calls == [{"lang": "deu", "timeout": 30}]


def test_missing_german_ocr_does_not_silently_use_english(tmp_path, monkeypatch):
    from openreader_backend.services import document_service as module

    monkeypatch.setattr(
        module.subprocess,
        "run",
        lambda *a, **k: SimpleNamespace(stdout="List of languages (1):\neng\n"),
    )
    with pytest.raises(ValueError, match="tesseract-ocr-deu"):
        DocumentService().extract_text(tmp_path / "unused.png", "German")


def test_ocr_timeout_propagates(tmp_path, monkeypatch):
    from openreader_backend.services import document_service as module

    path = tmp_path / "capture.png"
    Image.new("RGB", (100, 50), "white").save(path)
    monkeypatch.setattr(
        module.subprocess,
        "run",
        lambda *a, **k: SimpleNamespace(stdout="List of languages (1):\neng\n"),
    )

    def recognize(image, **kwargs):
        raise RuntimeError("Tesseract process timeout")

    monkeypatch.setattr(module.pytesseract, "image_to_string", recognize)
    with pytest.raises(RuntimeError, match="timeout"):
        DocumentService().extract_text(path, "English")


def test_unknown_ocr_language_is_explicit():
    with pytest.raises(ValueError, match="Sprachzuordnung"):
        DocumentService._ocr_language("unknown")
