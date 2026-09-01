from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator
from pathlib import Path
import shutil
import sys

from ...models import VoiceProfile
from ..base import AudioChunk
from ...utils.pronunciation import apply_pronunciations


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
        process.stdin.write(f"{spoken_text}\n".encode("utf-8"))
        await process.stdin.drain()
        process.stdin.close()

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

            await process.wait()
            if process.returncode != 0:
                stderr = await process.stderr.read() if process.stderr else b""
                raise RuntimeError(stderr.decode("utf-8", errors="ignore") or "Piper failed")
        finally:
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=0.6)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.wait()

    @staticmethod
    def _resolve_command() -> Path:
        venv_cli = Path(sys.executable).parent / "piper"
        if venv_cli.exists():
            return venv_cli

        if path := shutil.which("piper"):
            return Path(path)

        raise RuntimeError("Piper CLI not found. Run `./scripts/setup-tts.sh --piper`.")
