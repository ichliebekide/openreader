from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

from .models import PlaybackEvent, SelectionEvent

ReaderEvent = SelectionEvent | PlaybackEvent


class EventHub:
    def __init__(self) -> None:
        self._subscribers: set[asyncio.Queue[ReaderEvent]] = set()

    async def publish(self, event: ReaderEvent) -> None:
        for queue in self._subscribers:
            if queue.full():
                queue.get_nowait()
            queue.put_nowait(event)

    async def subscribe(self) -> AsyncIterator[ReaderEvent]:
        queue: asyncio.Queue[ReaderEvent] = asyncio.Queue(maxsize=16)
        self._subscribers.add(queue)
        try:
            while True:
                yield await queue.get()
        finally:
            self._subscribers.discard(queue)
