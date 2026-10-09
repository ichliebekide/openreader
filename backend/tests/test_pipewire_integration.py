"""Opt-in local PipeWire checks; only silence is sent to the default output."""

import asyncio
import os
from contextlib import suppress
from io import BytesIO
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

from openreader_backend.tts.base import AudioChunk
from openreader_backend.tts.player import AudioPlayer

pytestmark = pytest.mark.skipif(
    os.getenv("OPENREADER_TEST_PIPEWIRE") != "1",
    reason="requires a live PipeWire session",
)


@pytest.mark.parametrize("encoding", ["raw_s16le", "wav"])
def test_real_pipewire_pause_resume_stop(encoding):
    async def run():
        data = b"\0\0" * (24000 * 4)
        if encoding == "wav":
            wav = BytesIO()
            sf.write(wav, np.zeros(24000 * 4), 24000, format="WAV")
            data = wav.getvalue()
        player = AudioPlayer()
        task = asyncio.create_task(player.play(AudioChunk(data, 24000, encoding)))
        try:
            async with asyncio.timeout(3):
                while player._raw_process is None:
                    if task.done():
                        await task
                        pytest.fail("player exited before pause")
                    await asyncio.sleep(0.01)
            process = player._raw_process
            await asyncio.sleep(0.1)
            await player.pause()
            await asyncio.sleep(0.15)
            status = Path(f"/proc/{process.pid}/status").read_text()
            assert "State:\tT" in status
            assert not task.done()
            await player.resume()
            await asyncio.sleep(0.15)
            assert "State:\tT" not in Path(f"/proc/{process.pid}/status").read_text()
            await player.pause()
        finally:
            task.cancel()
            with suppress(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
        assert process.returncode is not None
        assert player._raw_process is None and not player.paused

    asyncio.run(run())


def test_real_pipewire_resume_completes_eof_drain():
    async def run():
        player = AudioPlayer()
        task = asyncio.create_task(player.play(AudioChunk(b"\0\0" * 24000, 24000, "raw_s16le")))
        try:
            async with asyncio.timeout(3):
                while player._raw_process is None:
                    if task.done():
                        await task
                        pytest.fail("player exited before pause")
                    await asyncio.sleep(0.01)
            await player.pause()
            await asyncio.sleep(0.2)
            await player.resume()
            await asyncio.wait_for(task, 5)
            assert player._raw_process is None
        finally:
            if not task.done():
                task.cancel()
                with suppress(asyncio.CancelledError):
                    await task

    asyncio.run(run())
