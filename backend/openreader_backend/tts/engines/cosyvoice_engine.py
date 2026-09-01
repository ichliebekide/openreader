from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator
import os
from pathlib import Path
import shutil
import tempfile

import numpy as np
import soundfile as sf

from ...models import VoiceProfile, default_cosyvoice_binary_path
from ...utils.pronunciation import apply_pronunciations
from ..base import AudioChunk


class CosyVoiceEngine:
    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        chunk = await self._synthesize(text, profile)
        yield chunk

    async def _synthesize(self, text: str, profile: VoiceProfile) -> AudioChunk:
        cli = self.resolve_cli(profile)
        model = self.resolve_model(profile)
        prompt_speech = self.resolve_prompt_speech(profile)

        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            output = Path(tmp.name)

        args = [
            str(cli),
            "--model",
            str(model),
            "--prompt-speech",
            str(prompt_speech),
            "--text",
            apply_pronunciations(text.strip()),
            "--output",
            str(output),
            "--mode",
            self._mode(profile),
            "--speed",
            str(max(0.2, float(profile.length_scale or 1.0))),
            "--threads",
            str(max(1, min((os.cpu_count() or 4) // 2, 12))),
            "--quiet",
        ]
        instruction = self._instruction(profile)
        if instruction:
            args.extend(["--instruction", instruction])

        env = self._runtime_env(cli.parent, profile)
        process: asyncio.subprocess.Process | None = None
        try:
            process = await asyncio.create_subprocess_exec(
                *args,
                cwd=cli.parent,
                env=env,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await process.communicate()
            if process.returncode != 0:
                message = stderr.decode("utf-8", errors="ignore") or stdout.decode(
                    "utf-8",
                    errors="ignore",
                )
                raise RuntimeError(message.strip() or "CosyVoice CLI failed")

            return await asyncio.to_thread(self._read_audio_chunk, output, float(profile.volume or 1.0))
        except asyncio.CancelledError:
            if process and process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=0.8)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.wait()
            raise
        finally:
            output.unlink(missing_ok=True)

    @staticmethod
    def _read_audio_chunk(output: Path, volume: float) -> AudioChunk:
        audio, sample_rate = sf.read(output, dtype="float32")
        audio = np.asarray(audio)
        if audio.ndim > 1:
            audio = audio.mean(axis=1)
        audio = np.clip(audio * volume, -1.0, 1.0)
        raw = (audio * 32767.0).astype(np.int16).tobytes()
        return AudioChunk(data=raw, sample_rate=int(sample_rate), encoding="raw_s16le")

    @staticmethod
    def _runtime_env(runtime_dir: Path, profile: VoiceProfile) -> dict[str, str]:
        env = dict(os.environ)
        library_paths = [str(runtime_dir)]
        if profile.cosyvoice_backend_path and profile.cosyvoice_backend_path.exists():
            library_paths.append(str(profile.cosyvoice_backend_path))
        if current := env.get("LD_LIBRARY_PATH"):
            library_paths.append(current)
        env["LD_LIBRARY_PATH"] = ":".join(dict.fromkeys(library_paths))
        return env

    @staticmethod
    def _mode(profile: VoiceProfile) -> str:
        mode = (profile.cosyvoice_mode or "instruct").strip().lower()
        if mode in {"zero-shot", "instruct", "cross-lingual"}:
            return mode
        return "instruct"

    @staticmethod
    def _instruction(profile: VoiceProfile) -> str | None:
        if CosyVoiceEngine._mode(profile) != "instruct":
            return None
        if profile.cosyvoice_instruction:
            return profile.cosyvoice_instruction
        language = (profile.language or "").strip().lower()
        if language in {"de", "de-de", "de_de", "deutsch", "german"}:
            return "Bitte sprich den Text auf Deutsch mit klarer, natuerlicher deutscher Aussprache."
        if language in {"en", "en-us", "en_gb", "english"}:
            return "Please speak the text in clear, natural English."
        return None

    @staticmethod
    def resolve_cli(profile: VoiceProfile) -> Path:
        candidates: list[Path | None] = []
        if profile.cosyvoice_binary_path:
            binary = profile.cosyvoice_binary_path
            candidates.extend(
                [
                    binary if binary.name == "cosyvoice-cli" else None,
                    binary.with_name("cosyvoice-cli"),
                ]
            )

        default_binary = default_cosyvoice_binary_path()
        candidates.append(default_binary.with_name("cosyvoice-cli"))
        candidates.append(Path(path) if (path := shutil.which("cosyvoice-cli")) else None)

        for candidate in candidates:
            if candidate and candidate.exists():
                return candidate
        raise RuntimeError("CosyVoice CLI missing; run `./scripts/setup-cosyvoice.sh`.")

    @staticmethod
    def resolve_binary(profile: VoiceProfile) -> Path:
        return CosyVoiceEngine.resolve_cli(profile)

    @staticmethod
    def resolve_model(profile: VoiceProfile) -> Path:
        if profile.cosyvoice_model_path and profile.cosyvoice_model_path.exists():
            return profile.cosyvoice_model_path
        raise RuntimeError("CosyVoice model missing; run `./scripts/setup-cosyvoice.sh`.")

    @staticmethod
    def resolve_prompt_speech(profile: VoiceProfile) -> Path:
        if profile.cosyvoice_prompt_speech_path and profile.cosyvoice_prompt_speech_path.exists():
            return profile.cosyvoice_prompt_speech_path
        raise RuntimeError("CosyVoice prompt_speech missing; run `./scripts/setup-cosyvoice.sh`.")
