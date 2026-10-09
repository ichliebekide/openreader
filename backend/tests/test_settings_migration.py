import asyncio
import json

import pytest
from pydantic import ValidationError

from openreader_backend.models import ReaderSettings, TTSEngine, VoiceProfile
from openreader_backend.settings import SettingsStore
from openreader_backend.tts.orchestrator import TTSOrchestrator


@pytest.mark.parametrize("active", ["qwen", "cosyvoice", "mock", "piper", "edge"])
def test_old_settings_preserve_surviving_profiles_and_preferences(tmp_path, active):
    path = tmp_path / "settings.json"
    data = {
        "backend_engine": active,
        "active_profile_id": active,
        "min_selection_chars": 17,
        "overlay_timeout_ms": 9000,
        "qwen_device": "cuda",
        "voice_profiles": [
            {"id": engine, "engine": engine, "volume": 0.63, "length_scale": 1.2,
             "label": f"Custom {engine}", "edge_voice": "de-DE-SeraphinaMultilingualNeural",
             "piper_model_path": "/custom/model.onnx", "reference_audio": "old.wav"}
            for engine in ["qwen", "cosyvoice", "mock", "piper", "edge"]
        ],
    }
    original = json.dumps(data)
    path.write_text(original)
    store = SettingsStore(path)
    settings = store.settings
    expected = active if active in ("piper", "edge") else "piper"
    assert settings.backend_engine == expected and settings.active_profile_id == expected
    assert settings.min_selection_chars == 17 and settings.overlay_timeout_ms == 9000
    assert {p.engine for p in settings.voice_profiles} == {TTSEngine.PIPER, TTSEngine.EDGE}
    for engine in ("edge", "piper"):
        profile = next(p for p in settings.voice_profiles if p.id == engine)
        assert profile.volume == 0.63 and profile.length_scale == 1.2
        assert str(profile.piper_model_path) == "/custom/model.onnx"
    assert path.read_text() == original  # Loading does not destructively rewrite the old file.
    store.update(settings)
    saved = json.loads(path.read_text())
    assert "qwen_device" not in saved
    assert all("reference_audio" not in p for p in saved["voice_profiles"])
    assert SettingsStore(path).settings == settings


def test_new_install_is_offline_and_has_only_supported_engines(tmp_path):
    settings = SettingsStore(tmp_path / "missing.json").settings
    assert settings.backend_engine == TTSEngine.PIPER
    active = next(p for p in settings.voice_profiles if p.id == settings.active_profile_id)
    assert active.engine == TTSEngine.PIPER
    orchestrator = TTSOrchestrator(settings)
    assert set(orchestrator.engines) == {TTSEngine.EDGE, TTSEngine.PIPER}
    assert {s.engine for s in orchestrator.status().engines} == set(orchestrator.engines)
    assert orchestrator.status().fallback_order == [TTSEngine.PIPER]
    for profile in settings.voice_profiles:
        candidates = orchestrator._candidate_profiles(profile)
        assert all(p.engine == TTSEngine.PIPER for p in candidates[1:])


@pytest.mark.parametrize("engine", ["qwen", "cosyvoice", "mock"])
def test_retired_engines_cannot_be_reintroduced_through_api_schema(engine):
    with pytest.raises(ValidationError):
        ReaderSettings(backend_engine=engine)
    with pytest.raises(ValidationError):
        VoiceProfile(engine=engine)


def test_all_engines_failing_returns_error_instead_of_test_audio():
    async def run():
        profile = VoiceProfile(id="local", engine=TTSEngine.PIPER)
        tts = TTSOrchestrator(ReaderSettings(voice_profiles=[profile]))

        class BrokenEngine:
            async def synthesize(self, text, profile):
                raise RuntimeError("voice unavailable")
                yield

        tts.engines[TTSEngine.PIPER] = BrokenEngine()
        with pytest.raises(RuntimeError, match="voice unavailable"):
            async for _ in tts._synthesize_sections("Hallo", profile):
                pytest.fail("Unexpected audio after engine failure")
    asyncio.run(run())
