from __future__ import annotations

import subprocess
import unicodedata
from pathlib import Path

import pytesseract
from bs4 import BeautifulSoup
from ebooklib import ITEM_DOCUMENT, epub
from PIL import Image, ImageOps
from pypdf import PdfReader

from ..utils.text import normalize_selection


class DocumentService:
    def extract_text(self, path: Path, language: str = "German") -> str:
        suffix = path.suffix.lower()
        if suffix == ".pdf":
            return self._extract_pdf(path)
        if suffix == ".epub":
            return self._extract_epub(path)
        if suffix in {".png", ".jpg", ".jpeg", ".tif", ".tiff", ".webp"}:
            return self._extract_ocr(path, language)
        if suffix in {".txt", ".md"}:
            data = path.read_bytes()
            encoding = "utf-16" if data.startswith((b"\xff\xfe", b"\xfe\xff")) else "utf-8-sig"
            try:
                return normalize_selection(data.decode(encoding))
            except UnicodeDecodeError as error:
                raise ValueError(
                    "Die Textdatei muss UTF-8 oder UTF-16 mit BOM verwenden."
                ) from error
        if suffix in {".html", ".htm", ".xhtml"}:
            return self._html_text(path.read_bytes())
        raise ValueError(f"Unsupported document type: {suffix}")

    @staticmethod
    def _extract_pdf(path: Path) -> str:
        reader = PdfReader(str(path))
        text = normalize_selection("\n".join(page.extract_text() or "" for page in reader.pages))
        if not text:
            raise ValueError(
                "PDF ohne Textschicht: Bitte die gescannten Seiten als Bilder importieren."
            )
        return text

    @staticmethod
    def _extract_epub(path: Path) -> str:
        book = epub.read_epub(str(path))
        parts: list[str] = []
        if book.spine:
            items = [book.get_item_with_id(uid) for uid, linear in book.spine if linear != "no"]
        else:
            items = list(book.get_items_of_type(ITEM_DOCUMENT))
        for item in items:
            if item and item.get_type() == ITEM_DOCUMENT and "nav" not in item.properties:
                parts.append(DocumentService._html_text(item.get_content()))
        return normalize_selection("\n".join(parts))

    @staticmethod
    def _html_text(data: bytes) -> str:
        soup = BeautifulSoup(data, "html.parser")
        for element in soup.select("script, style, nav, head, [hidden], [aria-hidden='true']"):
            element.decompose()
        return normalize_selection(soup.get_text(" "))

    @staticmethod
    def _ocr_language(language: str) -> str:
        aliases = {
            "german": "deu",
            "deutsch": "deu",
            "de": "deu",
            "deu": "deu",
            "english": "eng",
            "en": "eng",
            "eng": "eng",
            "french": "fra",
            "fr": "fra",
            "fra": "fra",
            "spanish": "spa",
            "es": "spa",
            "spa": "spa",
            "italian": "ita",
            "it": "ita",
            "ita": "ita",
            "portuguese": "por",
            "pt": "por",
            "por": "por",
            "japanese": "jpn",
            "ja": "jpn",
            "jpn": "jpn",
            "korean": "kor",
            "ko": "kor",
            "kor": "kor",
            "chinese": "chi_sim",
            "zh": "chi_sim",
            "chi_sim": "chi_sim",
            "russian": "rus",
            "ru": "rus",
            "rus": "rus",
        }
        key = language.lower().replace("_", "-").split("-")[0]
        code = aliases.get(language.lower()) or aliases.get(key)
        if code is None:
            raise ValueError(f"Keine OCR-Sprachzuordnung fuer das Voice-Profil: {language}")
        return code

    @staticmethod
    def _extract_ocr(path: Path, language: str) -> str:
        code = DocumentService._ocr_language(language)
        try:
            result = subprocess.run(
                [pytesseract.pytesseract.tesseract_cmd, "--list-langs"],
                capture_output=True,
                text=True,
                check=True,
                timeout=5,
            )
        except (OSError, subprocess.SubprocessError) as error:
            raise RuntimeError("Tesseract ist nicht verfuegbar oder antwortet nicht.") from error
        installed = set(result.stdout.splitlines()[1:])
        if code not in installed:
            raise ValueError(f"OCR-Sprachdaten fehlen: {code}. Installiere tesseract-ocr-{code}.")
        with Image.open(path) as image:
            if image.width * image.height > 40_000_000:
                raise ValueError("OCR-Bild zu gross (maximal 40 Megapixel). Bitte zuschneiden.")
            prepared = ImageOps.exif_transpose(image).convert("RGBA")
            background = Image.new("RGBA", prepared.size, "white")
            background.alpha_composite(prepared)
            prepared = ImageOps.grayscale(background)
            # Improve small screenshot text without unbounded upscaling of large scans.
            if max(prepared.size) < 2000 and prepared.width * prepared.height <= 3_000_000:
                prepared = prepared.resize(
                    (prepared.width * 2, prepared.height * 2),
                    Image.Resampling.LANCZOS,
                )
            text = pytesseract.image_to_string(prepared, lang=code, timeout=30)
            return normalize_selection(unicodedata.normalize("NFC", text))
