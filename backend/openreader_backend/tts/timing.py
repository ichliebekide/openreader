from __future__ import annotations

from .base import WordBoundary


class WordMatcher:
    """Map service words to the original text, using browser UTF-16 offsets."""

    def __init__(self, text: str) -> None:
        self.text = text
        self.cursor = 0
        self.utf16_cursor = 0

    def match(self, word: str, offset: int, duration: int) -> WordBoundary | None:
        if not word:
            return None
        start = self.text.find(word, self.cursor)
        if start < 0:
            return None
        end = start + len(word)
        utf16_start = self.utf16_cursor + len(
            self.text[self.cursor:start].encode("utf-16-le")
        ) // 2
        utf16_end = utf16_start + len(word.encode("utf-16-le")) // 2
        self.cursor = end
        self.utf16_cursor = utf16_end
        return WordBoundary(utf16_start, utf16_end, offset / 10_000_000, duration / 10_000_000)
