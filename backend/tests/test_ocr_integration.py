import os

import pytest
from PIL import Image, ImageDraw, ImageFont

from openreader_backend.services.document_service import DocumentService


@pytest.mark.skipif(os.getenv("OPENREADER_TEST_OCR") != "1", reason="requires Tesseract eng")
def test_real_tesseract_reads_prepared_image(tmp_path):
    path = tmp_path / "text.png"
    image = Image.new("RGB", (1400, 160), "white")
    font = ImageFont.truetype("DejaVuSans.ttf", 40)
    ImageDraw.Draw(image).text((30, 40), "OpenReader reads selected text.", font=font, fill="black")
    image.save(path)
    text = DocumentService().extract_text(path, "English")
    assert text == "OpenReader reads selected text."
