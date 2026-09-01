from __future__ import annotations

import re

from ..models import DesktopContextSnapshot, ReaderSettings

SECRET_SHAPED_RE = re.compile(r"^[A-Za-z0-9_./+=:-]{6,}$")


class SelectionGuard:
    def __init__(self, settings: ReaderSettings) -> None:
        self.settings = settings

    def accepts(self, text: str, context: DesktopContextSnapshot) -> bool:
        if len(text.strip()) < self.settings.min_selection_chars:
            return False

        if self.settings.ignore_terminal_windows and self._is_excluded_app(context):
            return False

        if self._looks_like_secret(text):
            return False

        return True

    def _is_excluded_app(self, context: DesktopContextSnapshot) -> bool:
        active = (context.active_resource_class or context.active_resource_name or "").lower()
        return any(token.lower() in active for token in self.settings.excluded_window_classes)

    @staticmethod
    def _looks_like_secret(text: str) -> bool:
        clean = text.strip()
        if len(clean.split()) > 3:
            return False
        if " " in clean:
            return False
        return bool(SECRET_SHAPED_RE.match(clean)) and any(char.isdigit() for char in clean)
