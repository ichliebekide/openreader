from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

from .models import SelectionEvent


class EventHub:
    def __init__(self) -> None:
        self._subscribers: set[asyncio.Queue[SelectionEvent]] = set()

    async def publish(self, event: SelectionEvent) -> None:
        dead: list[asyncio.Queue[SelectionEvent]] = []
        for queue in self._subscribers:
            try:
                queue.put_nowait(event)
            except asyncio.QueueFull:
                dead.append(queue)

        for queue in dead:
            self._subscribers.discard(queue)

    async def subscribe(self) -> AsyncIterator[SelectionEvent]:
        queue: asyncio.Queue[SelectionEvent] = asyncio.Queue(maxsize=16)
        self._subscribers.add(queue)
        try:
            while True:
                yield await queue.get()
        finally:
            self._subscribers.discard(queue)
