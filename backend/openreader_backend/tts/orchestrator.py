from __future__ import annotations

import logging
from io import BytesIO
import importlib.util
from pathlib import Path
import shutil

import numpy as np
import soundfile as sf

from ..models import ReaderSettings, TTSEngine, TTSEngineStatus, TTSStatusResponse, VoiceProfile
from ..utils.text import split_sentences
from .engines.cosyvoice_engine import CosyVoiceEngine
from .engines.edge_engine import EdgeEngine
from .engines.mock_engine import MockEngine
from .engines.piper_engine import PiperEngine
from .engines.qwen_engine import QwenEngine
from .player import AudioPlayer

logger = logging.getLogger(__name__)


class TTSOrchestrator:
    def __init__(self, settings: ReaderSettings) -> None:
        self.settings = settings
        self.player = AudioPlayer()
        self.engines = {
            TTSEngine.QWEN: QwenEngine(settings),
            TTSEngine.COSYVOICE: CosyVoiceEngine(),
            TTSEngine.EDGE: EdgeEngine(),
            TTSEngine.PIPER: PiperEngine(),
            TTSEngine.MOCK: MockEngine(),
        }

    async def speak(self, text: str, profile_id: str | None = None) -> None:
        profile = self._profile(profile_id)
        for sentence in self._split_for_profile(text, profile):
            try:
                await self._speak_sentence(sentence, profile)
            except Exception:
                logger.exception("All TTS engines failed for sentence")
                raise

    async def stop(self) -> None:
        await self.player.stop()

    async def export_wav(self, text: str, output_path: Path, profile_id: str | None = None) -> Path:
        profile = self._profile(profile_id)
        engine = self.engines[profile.engine]
        output_path.parent.mkdir(parents=True, exist_ok=True)
        samples: list[np.ndarray] = []
        sample_rate = profile.piper_sample_rate

        for sentence in self._split_for_profile(text, profile):
            async for chunk in engine.synthesize(sentence, profile):
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

    async def warmup_active(self) -> None:
        profile = self._profile(None)
        engine = self.engines[profile.engine]
        warmup = getattr(engine, "warmup", None)
        if not warmup:
            return

        try:
            await warmup(profile)
            logger.info("%s TTS warmup completed", profile.engine)
        except Exception:
            logger.exception("%s TTS warmup failed", profile.engine)

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

    async def _speak_sentence(self, sentence: str, preferred_profile: VoiceProfile) -> None:
        last_error: Exception | None = None
        for profile in self._candidate_profiles(preferred_profile):
            try:
                await self.player.play_stream(self.engines[profile.engine].synthesize(sentence, profile))
                return
            except Exception as error:
                last_error = error
                logger.exception("TTS engine %s failed; trying fallback", profile.engine)

        if last_error:
            raise last_error

    def _candidate_profiles(self, preferred_profile: VoiceProfile) -> list[VoiceProfile]:
        profiles = [preferred_profile]
        profiles.extend(
            profile
            for engine in [TTSEngine.COSYVOICE, TTSEngine.PIPER, TTSEngine.MOCK]
            for profile in self.settings.voice_profiles
            if profile.engine == engine and profile.id != preferred_profile.id
        )

        if not any(profile.engine == TTSEngine.MOCK for profile in profiles):
            profiles.append(VoiceProfile(id="mock", label="Mock Dev", engine=TTSEngine.MOCK))

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
        if profile.engine == TTSEngine.QWEN:
            max_chars = 180
        elif profile.engine == TTSEngine.COSYVOICE:
            max_chars = 220
        elif profile.engine == TTSEngine.EDGE:
            max_chars = 300
        else:
            max_chars = 420
        return split_sentences(text, max_chars=max_chars)

    def status(self) -> TTSStatusResponse:
        qwen_profile = self._first_profile(TTSEngine.QWEN)
        cosyvoice_profile = self._first_profile(TTSEngine.COSYVOICE)
        edge_profile = self._first_profile(TTSEngine.EDGE)
        piper_profile = self._first_profile(TTSEngine.PIPER)
        engines = [
            self._qwen_status(qwen_profile),
            self._cosyvoice_status(cosyvoice_profile),
            self._edge_status(edge_profile),
            self._piper_status(piper_profile),
            TTSEngineStatus(
                engine=TTSEngine.MOCK,
                available=True,
                configured=True,
                detail="Development fallback audio generator",
            ),
        ]
        return TTSStatusResponse(
            preferred_engine=self.settings.backend_engine,
            engines=engines,
            fallback_order=list(
                dict.fromkeys(
                    [
                        self.settings.backend_engine,
                        TTSEngine.COSYVOICE,
                        TTSEngine.PIPER,
                        TTSEngine.MOCK,
                    ]
                )
            ),
        )

    def _first_profile(self, engine: TTSEngine) -> VoiceProfile | None:
        return next((profile for profile in self.settings.voice_profiles if profile.engine == engine), None)

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
    def _qwen_status(profile: VoiceProfile | None) -> TTSEngineStatus:
        installed = (
            importlib.util.find_spec("torch") is not None
            and importlib.util.find_spec("qwen_tts") is not None
        )
        sox_ok = shutil.which("sox") is not None
        configured = bool(profile and profile.reference_audio and profile.reference_text)
        if not installed:
            detail = "torch/qwen_tts not installed; run `pip install -e backend[qwen]`"
        elif not sox_ok:
            detail = "qwen-tts is installed, but system `sox` is missing; run `sudo apt install sox`"
        elif not configured:
            detail = "Reference audio and reference text are required for the Base model"
        else:
            detail = "Ready"
        return TTSEngineStatus(
            engine=TTSEngine.QWEN,
            available=installed and sox_ok,
            configured=configured and sox_ok,
            detail=detail,
        )

    @staticmethod
    def _cosyvoice_status(profile: VoiceProfile | None) -> TTSEngineStatus:
        cli_ok = False
        if profile:
            try:
                CosyVoiceEngine.resolve_cli(profile)
                cli_ok = True
            except Exception:
                cli_ok = False

        model_ok = bool(profile and profile.cosyvoice_model_path and profile.cosyvoice_model_path.exists())
        prompt_ok = bool(
            profile
            and profile.cosyvoice_prompt_speech_path
            and profile.cosyvoice_prompt_speech_path.exists()
        )

        if not cli_ok:
            detail = "CosyVoice runtime missing; run `./scripts/setup-cosyvoice.sh`"
        elif not model_ok:
            detail = "CosyVoice GGUF model missing; run `./scripts/setup-cosyvoice.sh`"
        elif not prompt_ok:
            detail = "CosyVoice prompt_speech missing; run `./scripts/setup-cosyvoice.sh`"
        else:
            detail = str(profile.cosyvoice_model_path)

        return TTSEngineStatus(
            engine=TTSEngine.COSYVOICE,
            available=cli_ok and model_ok and prompt_ok,
            configured=model_ok and prompt_ok,
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
            profile
            and profile.piper_config_path
            and profile.piper_config_path.exists()
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
