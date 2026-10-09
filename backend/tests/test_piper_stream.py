import asyncio
import sys

import pytest

from openreader_backend.models import TTSEngine, VoiceProfile
from openreader_backend.tts.engines.piper_engine import PiperEngine


@pytest.mark.parametrize('cancel', [False, True])
def test_piper_uses_one_child_and_drains_both_output_pipes(monkeypatch, tmp_path, cancel):
    async def run():
        model = tmp_path / 'model.onnx'
        model.touch()
        profile = VoiceProfile(engine=TTSEngine.PIPER, piper_model_path=model)
        processes = []
        spawn = asyncio.create_subprocess_exec
        # Behaves like a streaming CLI, without downloading or loading a voice.
        cli = (
            'import sys\n'
            'for line in sys.stdin:\n'
            ' sys.stderr.write("diagnostic" * 16000)\n'
            ' sys.stderr.flush()\n'
            ' sys.stdout.buffer.write(bytes(480))\n'
            ' sys.stdout.buffer.flush()\n'
        )
        async def create(*args, **kwargs):
            process = await spawn(sys.executable, '-c', cli, **kwargs)
            processes.append(process)
            return process
        monkeypatch.setattr(asyncio, 'create_subprocess_exec', create)
        monkeypatch.setattr(PiperEngine, '_resolve_command', staticmethod(lambda: sys.executable))
        stream = PiperEngine().synthesize('Ein Testsatz. ' * 5000, profile)
        data = 0
        try:
            async with asyncio.timeout(5):
                async for chunk in stream:
                    data += len(chunk.data)
                    if cancel:
                        break
        finally:
            await stream.aclose()
        assert len(processes) == 1 and processes[0].returncode is not None
        assert data > 0
        if not cancel:
            assert data > 480
    asyncio.run(run())
