from __future__ import annotations

import html
import re

WHITESPACE_RE = re.compile(r"\s+")
SENTENCE_BOUNDARY_RE = re.compile(r'(?<=[.!?。！？])(?:["\u201c\u201d\u00bb\u2019\')\]]*)\s+')
UNICODE_ESCAPE_RE = re.compile(r"\\u([0-9a-fA-F]{4})|\\U([0-9a-fA-F]{8})")


def normalize_selection(text: str) -> str:
    clean = decode_unicode_escapes(text.replace("\x00", ""))
    clean = html.unescape(clean)
    return WHITESPACE_RE.sub(" ", clean).strip()


def decode_unicode_escapes(text: str) -> str:
    if "\\u" not in text and "\\U" not in text:
        return text

    def replace(match: re.Match[str]) -> str:
        value = match.group(1) or match.group(2)
        try:
            return chr(int(value, 16))
        except (TypeError, ValueError):
            return match.group(0)

    return UNICODE_ESCAPE_RE.sub(replace, text)


def preview(text: str, max_chars: int = 520) -> str:
    clean = normalize_selection(text)
    if len(clean) <= max_chars:
        return clean
    return f"{clean[: max_chars - 1]}…"


def split_sentences(text: str, max_chars: int = 420) -> list[str]:
    if max_chars < 1:
        raise ValueError("max_chars must be positive")
    clean = normalize_selection(text)
    if not clean:
        return []

    sentences: list[str] = []
    buffer = ""
    # Slice the original text instead of dropping punctuation or splitting decimals.
    start = 0
    parts: list[str] = []
    for match in SENTENCE_BOUNDARY_RE.finditer(clean):
        end = match.end()
        parts.append(clean[start:end].strip())
        start = end
    parts.append(clean[start:])
    for part in parts:
        if not part:
            continue

        if len(part) > max_chars:
            if buffer:
                sentences.append(buffer)
                buffer = ""
            sentences.extend(split_long_text(part, max_chars))
            continue

        if len(buffer) + len(part) + 1 <= max_chars:
            buffer = f"{buffer} {part}".strip()
            continue

        if buffer:
            sentences.append(buffer)
        buffer = part

    if buffer:
        sentences.append(buffer)

    return sentences or [clean]


def split_long_text(text: str, max_chars: int) -> list[str]:
    if max_chars < 1:
        raise ValueError("max_chars must be positive")
    chunks: list[str] = []
    buffer = ""

    for word in text.split(" "):
        if not word:
            continue

        if len(word) > max_chars:
            if buffer:
                chunks.append(buffer)
                buffer = ""
            chunks.extend(word[i : i + max_chars] for i in range(0, len(word), max_chars))
            continue

        candidate = f"{buffer} {word}".strip()
        if len(candidate) <= max_chars:
            buffer = candidate
            continue

        if buffer:
            chunks.append(buffer)
        buffer = word

    if buffer:
        chunks.append(buffer)

    return chunks
