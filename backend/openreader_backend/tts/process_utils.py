from __future__ import annotations

import asyncio


async def read_error_tail(stream: asyncio.StreamReader | None) -> bytes:
    """Drain stderr concurrently without retaining unlimited diagnostic output."""
    tail = b""
    if stream:
        while data := await stream.read(4096):
            tail = (tail + data)[-8192:]
    return tail
