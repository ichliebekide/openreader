from __future__ import annotations

import importlib.util
import logging
import shutil
from collections.abc import AsyncIterator
from contextlib import aclosing
from io import BytesIO
from pathlib import Path

import numpy as np
import soundfile as sf

from ..models import ReaderSettings, TTSEngine, TTSEngineStatus, TTSStatusResponse, VoiceProfile
from ..utils.text import normalize_selection
from .base import AudioChunk
from .clocked_player import ClockedAudioPlayer, ProgressCallback
from .engines.edge_engine import EdgeEngine
from .engines.piper_engine import PiperEngine
from .player import AudioPlayer

logger = logging.getLogger(__name__)


class TTSOrchestrator:
    def __init__(
        self, settings: ReaderSettings, on_progress: ProgressCallback | None = None
    ) -> None:
        self.settings = settings
        self.player = AudioPlayer()
        self.on_progress = on_progress
        self.engines = {
            TTSEngine.EDGE: EdgeEngine(),
            TTSEngine.PIPER: PiperEngine(),
        }

    async def speak(self, text: str, profile_id: str | None = None) -> None:
        profile = self._profile(profile_id)
        if self.on_progress:
            paused = self.player.paused
            self.player = (
                ClockedAudioPlayer(self.on_progress)
                if profile.engine == TTSEngine.EDGE else AudioPlayer()
            )
            if paused:
                await self.player.pause()
        try:
            await self.player.play_stream(self._synthesize_sections(text, profile))
        except Exception:
            logger.exception("All TTS engines failed during playback")
            raise

    async def stop(self) -> None:
        await self.player.stop()

    async def pause(self) -> None:
        await self.player.pause()

    async def resume(self) -> None:
        await self.player.resume()

    @property
    def active_language(self) -> str:
        return self._profile(None).language

    async def export_wav(self, text: str, output_path: Path, profile_id: str | None = None) -> Path:
        profile = self._profile(profile_id)
        engine = self.engines[profile.engine]
        output_path.parent.mkdir(parents=True, exist_ok=True)
        samples: list[np.ndarray] = []
        sample_rate = profile.piper_sample_rate

        for sentence in self._split_for_profile(text, profile):
            async for chunk in engine.synthesize(sentence, profile):
                if not chunk.data:
                    continue
                sample_rate = chunk.sample_rate
                if chunk.encoding == "raw_s16le":
                    data = np.frombuffer(chunk.data, dtype=np.int16).astype(np.float32) / 32768.0
                else:
                    data, sample_rate = sf.read(BytesIO(chunk.data), dtype="float32")
                samples.append(np.asarray(data))

        if not samples:
            raise RuntimeError("No audio generated")

        sf.write(output_path, np.concatenate(samples), sample_rate)

        return output_path

    def _profile(self, profile_id: str | None) -> VoiceProfile:
        if profile_id:
            for profile in self.settings.voice_profiles:
                if profile.id == profile_id:
                    return profile

        if self.settings.active_profile_id:
            for profile in self.settings.voice_profiles:
                if profile.id == self.settings.active_profile_id:
                    return profile

        for profile in self.settings.voice_profiles:
            if profile.engine == self.settings.backend_engine:
                return profile

        return VoiceProfile(engine=self.settings.backend_engine)

    async def _synthesize_sections(
        self,
        text: str,
        preferred_profile: VoiceProfile,
    ) -> AsyncIterator[AudioChunk]:
        for section in self._split_for_profile(text, preferred_profile):
            last_error: Exception | None = None
            for profile in self._candidate_profiles(preferred_profile):
                emitted_audio = False
                pending_words = []
                try:
                    async with aclosing(
                        self.engines[profile.engine].synthesize(section, profile)
                    ) as audio:
                        async for chunk in audio:
                            pending_words.extend(chunk.words)
                            if not chunk.data:
                                if emitted_audio and pending_words:
                                    yield chunk
                                    pending_words.clear()
                                continue
                            chunk.words = tuple(pending_words)
                            pending_words.clear()
                            emitted_audio = True
                            yield chunk
                    if not emitted_audio:
                        raise RuntimeError(f"TTS engine {profile.engine} returned no audio")
                    break
                except Exception as error:
                    if emitted_audio:
                        # Replaying the section with another voice would repeat spoken text.
                        raise
                    last_error = error
                    logger.exception("TTS engine %s failed; trying fallback", profile.engine)
            else:
                if last_error:
                    raise last_error

    def _candidate_profiles(self, preferred_profile: VoiceProfile) -> list[VoiceProfile]:
        profiles = [preferred_profile]
        profiles.extend(
            profile
            for engine in [TTSEngine.PIPER]
            for profile in self.settings.voice_profiles
            if profile.engine == engine and profile.id != preferred_profile.id
        )

        seen: set[str] = set()
        unique: list[VoiceProfile] = []
        for profile in profiles:
            key = f"{profile.engine}:{profile.id}"
            if key not in seen:
                unique.append(profile)
                seen.add(key)
        return unique

    @staticmethod
    def _split_for_profile(text: str, profile: VoiceProfile) -> list[str]:
        clean = normalize_selection(text)
        return [clean] if clean else []

    def status(self) -> TTSStatusResponse:
        return TTSStatusResponse(
            preferred_engine=self.settings.backend_engine,
            engines=[
                self._edge_status(self._first_profile(TTSEngine.EDGE)),
                self._piper_status(self._first_profile(TTSEngine.PIPER)),
            ],
            fallback_order=list(dict.fromkeys(
                [self.settings.backend_engine, TTSEngine.PIPER]
            )),
        )

    def _first_profile(self, engine: TTSEngine) -> VoiceProfile | None:
        return next(
            (profile for profile in self.settings.voice_profiles if profile.engine == engine), None
        )

    @staticmethod
    def _edge_status(profile: VoiceProfile | None) -> TTSEngineStatus:
        installed = importlib.util.find_spec("edge_tts") is not None
        ffmpeg_ok = shutil.which("ffmpeg") is not None
        configured = bool(profile and profile.edge_voice)
        if not installed:
            detail = "edge-tts is not installed"
        elif not ffmpeg_ok:
            detail = "ffmpeg is required for streaming playback"
        elif not configured:
            detail = "Microsoft voice name is missing"
        else:
            detail = f"{profile.edge_voice} (online)"
        return TTSEngineStatus(
            engine=TTSEngine.EDGE,
            available=installed and ffmpeg_ok,
            configured=configured,
            detail=detail,
        )

    @staticmethod
    def _piper_status(profile: VoiceProfile | None) -> TTSEngineStatus:
        command_ok = True
        try:
            PiperEngine._resolve_command()
        except Exception:
            command_ok = False

        model_ok = bool(profile and profile.piper_model_path and profile.piper_model_path.exists())
        config_ok = bool(
            profile and profile.piper_config_path and profile.piper_config_path.exists()
        )
        if not command_ok:
            detail = "Piper CLI not found; run `./scripts/setup-tts.sh --piper`"
        elif not model_ok:
            detail = "Piper voice model missing; run `./scripts/setup-tts.sh --piper`"
        elif not config_ok:
            detail = "Piper config JSON missing"
        else:
            detail = str(profile.piper_model_path)
        return TTSEngineStatus(
            engine=TTSEngine.PIPER,
            available=command_ok and model_ok,
            configured=model_ok and config_ok,
            detail=detail,
        )
