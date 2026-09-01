from __future__ import annotations

from pathlib import Path

from bs4 import BeautifulSoup
from ebooklib import ITEM_DOCUMENT, epub
from PIL import Image
from pypdf import PdfReader
import pytesseract

from ..utils.text import normalize_selection


class DocumentService:
    def extract_text(self, path: Path) -> str:
        suffix = path.suffix.lower()
        if suffix == ".pdf":
            return self._extract_pdf(path)
        if suffix == ".epub":
            return self._extract_epub(path)
        if suffix in {".png", ".jpg", ".jpeg", ".tif", ".tiff", ".webp"}:
            return self._extract_ocr(path)
        if suffix in {".txt", ".md"}:
            return normalize_selection(path.read_text(encoding="utf-8", errors="ignore"))
        raise ValueError(f"Unsupported document type: {suffix}")

    @staticmethod
    def _extract_pdf(path: Path) -> str:
        reader = PdfReader(str(path))
        return normalize_selection("\n".join(page.extract_text() or "" for page in reader.pages))

    @staticmethod
    def _extract_epub(path: Path) -> str:
        book = epub.read_epub(str(path))
        parts: list[str] = []
        for item in book.get_items():
            if item.get_type() == ITEM_DOCUMENT:
                soup = BeautifulSoup(item.get_content(), "html.parser")
                parts.append(soup.get_text(" "))
        return normalize_selection("\n".join(parts))

    @staticmethod
    def _extract_ocr(path: Path) -> str:
        with Image.open(path) as image:
            return normalize_selection(pytesseract.image_to_string(image))
