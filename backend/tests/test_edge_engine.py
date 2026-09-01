from openreader_backend.models import ReaderSettings, TTSEngine
from openreader_backend.tts.engines.edge_engine import EdgeEngine


def test_seraphina_is_available_as_default_profile() -> None:
    profile = next(
        profile
        for profile in ReaderSettings().voice_profiles
        if profile.id == "edge-de-seraphina"
    )

    assert profile.engine == TTSEngine.EDGE
    assert profile.edge_voice == "de-DE-SeraphinaMultilingualNeural"
    assert profile.language == "German"


def test_edge_rate_preserves_length_scale_semantics() -> None:
    assert EdgeEngine._rate(1.0) == "+0%"
    assert EdgeEngine._rate(0.8) == "+25%"
    assert EdgeEngine._rate(1.25) == "-20%"


def test_edge_volume_is_clamped_and_converted() -> None:
    assert EdgeEngine._volume(1.0) == "+0%"
    assert EdgeEngine._volume(0.75) == "-25%"
    assert EdgeEngine._volume(3.0) == "+100%"
