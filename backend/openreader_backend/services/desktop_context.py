from __future__ import annotations

import asyncio
from datetime import datetime, timezone
from contextlib import suppress

from ..models import DesktopContextSnapshot


class DesktopContext:
    def __init__(self) -> None:
        self._snapshot = DesktopContextSnapshot()
        self._lock = asyncio.Lock()

    async def update(
        self,
        cursor_x: int,
        cursor_y: int,
        resource_class: str = "",
        resource_name: str = "",
    ) -> None:
        async with self._lock:
            self._snapshot = DesktopContextSnapshot(
                cursor_x=cursor_x,
                cursor_y=cursor_y,
                active_resource_class=resource_class or None,
                active_resource_name=resource_name or None,
                updated_at=datetime.now(timezone.utc),
            )

    async def snapshot(self) -> DesktopContextSnapshot:
        async with self._lock:
            return self._snapshot.model_copy()


async def start_dbus_bridge(context: DesktopContext) -> None:
    try:
        from dbus_next.aio import MessageBus
        from dbus_next.service import ServiceInterface, method
    except Exception:
        return

    class OpenReaderDesktopInterface(ServiceInterface):
        def __init__(self) -> None:
            super().__init__("org.openreader.Desktop")

        @method()
        def ReportContext(
            self,
            cursor_x: "i",
            cursor_y: "i",
            resource_class: "s",
            resource_name: "s",
        ) -> "b":
            asyncio.create_task(
                context.update(cursor_x, cursor_y, resource_class, resource_name)
            )
            return True

    with suppress(Exception):
        bus = await MessageBus().connect()
        await bus.request_name("org.openreader.Desktop")
        bus.export("/org/openreader/Desktop", OpenReaderDesktopInterface())
        await bus.wait_for_disconnect()
