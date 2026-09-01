import asyncio

from openreader_backend.services.clipboard_monitor import ClipboardMonitor


class FakeKlipperInterface:
    def __init__(self) -> None:
        self.calls = 0

    async def call_get_clipboard_contents(self) -> str:
        self.calls += 1
        if self.calls < 3:
            return "bereits vorhandener Text"
        return "neu markierter Text"


def test_klipper_content_change_without_history_signal() -> None:
    async def run_test() -> None:
        monitor = ClipboardMonitor(None, None, None, debounce_ms=0)  # type: ignore[arg-type]
        monitor.debounce_seconds = 0
        published: list[str] = []

        async def capture_publish(text: str, _source: object) -> None:
            published.append(text)
            monitor._running = False

        monitor._publish = capture_publish  # type: ignore[method-assign]

        await asyncio.wait_for(
            monitor._monitor_klipper_contents(
                FakeKlipperInterface(),
                asyncio.Event(),
            ),
            timeout=1,
        )

        assert published == ["neu markierter Text"]

    asyncio.run(run_test())
