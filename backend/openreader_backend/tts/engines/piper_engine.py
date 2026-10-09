from __future__ import annotations

import asyncio
import shutil
import sys
from collections.abc import AsyncIterator
from contextlib import suppress
from pathlib import Path

from ...models import VoiceProfile
from ...utils.pronunciation import apply_pronunciations
from ...utils.text import split_sentences
from ..base import AudioChunk
from ..process_utils import read_error_tail


class PiperEngine:
    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        if not profile.piper_model_path:
            raise RuntimeError("Piper voice profile has no model path configured")
        if not profile.piper_model_path.exists():
            raise RuntimeError(f"Piper model not found: {profile.piper_model_path}")

        command = self._resolve_command()
        args = [
            str(command),
            "--model",
            str(profile.piper_model_path),
            "--output-raw",
            "--length-scale",
            str(profile.length_scale),
            "--noise-scale",
            str(profile.noise_scale),
            "--noise-w-scale",
            str(profile.noise_w_scale),
            "--volume",
            str(profile.volume),
        ]
        if profile.piper_config_path and profile.piper_config_path.exists():
            args.extend(["--config", str(profile.piper_config_path)])
        if profile.piper_speaker is not None:
            args.extend(["--speaker", str(profile.piper_speaker)])

        process = await asyncio.create_subprocess_exec(
            *args,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        assert process.stdin is not None
        assert process.stdout is not None
        spoken_text = apply_pronunciations(text.strip())
        async def feed() -> None:
            try:
                for section in split_sentences(spoken_text, max_chars=420):
                    process.stdin.write(f"{section}\n".encode())
                    await process.stdin.drain()
            finally:
                process.stdin.close()

        feeder = asyncio.create_task(feed())
        errors = asyncio.create_task(read_error_tail(process.stderr))

        try:
            while True:
                data = await process.stdout.read(8192)
                if not data:
                    break
                yield AudioChunk(
                    data=data,
                    sample_rate=profile.piper_sample_rate,
                    encoding="raw_s16le",
                )

            await feeder
            await process.wait()
            stderr = await errors
            if process.returncode != 0:
                raise RuntimeError(stderr.decode("utf-8", errors="ignore") or "Piper failed")
        finally:
            feeder.cancel()
            with suppress(asyncio.CancelledError, Exception):
                await feeder
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=0.6)
                except TimeoutError:
                    process.kill()
                    await process.wait()
            errors.cancel()
            with suppress(asyncio.CancelledError):
                await errors

    @staticmethod
    def _resolve_command() -> Path:
        venv_cli = Path(sys.executable).parent / "piper"
        if venv_cli.exists():
            return venv_cli

        if path := shutil.which("piper"):
            return Path(path)

        raise RuntimeError("Piper CLI not found. Run `./scripts/setup-tts.sh --piper`.")
