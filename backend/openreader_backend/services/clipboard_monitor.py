from __future__ import annotations

import asyncio
import os
import shutil
from asyncio import IncompleteReadError, LimitOverrunError
from collections.abc import Awaitable, Callable

from ..events import EventHub
from ..models import SelectionEvent, SelectionSource
from ..utils.text import normalize_selection, preview
from .desktop_context import DesktopContext
from .safety import SelectionGuard


class ClipboardMonitor:
    def __init__(
        self,
        hub: EventHub,
        context: DesktopContext,
        guard: SelectionGuard,
        debounce_ms: int,
    ) -> None:
        self.hub = hub
        self.context = context
        self.guard = guard
        self.debounce_seconds = max(debounce_ms / 1000, 0.55)
        self._last_text = ""
        self._observed_text = ""
        self._pending_publish: asyncio.Task[None] | None = None
        self._running = True

    async def run(self) -> None:
        if self._is_kde_session():
            await self._run_with_retry(self._watch_kde_selection)
            return

        if not shutil.which("wl-paste"):
            return

        await self._run_with_retry(
            lambda: self._watch_selection(SelectionSource.PRIMARY)
        )

    async def _watch_kde_selection(self) -> bool:
        if helper := os.getenv("OPENREADER_SELECTION_HELPER"):
            watched = await self._watch_native_selection(helper)
            if watched or not self._running:
                return watched

        return await self._watch_klipper()

    async def _watch_native_selection(self, helper: str) -> bool:
        try:
            process = await asyncio.create_subprocess_exec(
                helper,
                "--selection-helper",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL,
                limit=2 * 1024 * 1024,
            )
        except Exception:
            return False

        assert process.stdout is not None
        try:
            while self._running:
                try:
                    raw = await process.stdout.readuntil(b"\0")
                except (IncompleteReadError, LimitOverrunError):
                    return False

                text = normalize_selection(raw[:-1].decode("utf-8", errors="ignore"))
                self._schedule_candidate(
                    text,
                    SelectionSource.PRIMARY,
                    allow_duplicate=True,
                )

            return True
        finally:
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=0.4)
                except Exception:
                    process.kill()
                    await process.wait()

    async def _run_with_retry(self, watch: Callable[[], Awaitable[bool]]) -> None:
        retry_delay = 1.0
        try:
            while self._running:
                watched = await watch()
                if watched or not self._running:
                    retry_delay = 1.0
                    continue

                await asyncio.sleep(retry_delay)
                retry_delay = min(retry_delay * 2, 30.0)
        finally:
            self._cancel_pending_publish()

    async def _watch_klipper(self) -> bool:
        try:
            from dbus_next.aio import MessageBus

            bus = await MessageBus().connect()
            introspection = await bus.introspect("org.kde.klipper", "/klipper")
            proxy = bus.get_proxy_object("org.kde.klipper", "/klipper", introspection)
            interface = proxy.get_interface("org.kde.klipper.klipper")
        except asyncio.CancelledError:
            raise
        except Exception:
            return False

        updated = asyncio.Event()

        def selection_changed() -> None:
            updated.set()

        interface.on_clipboard_history_updated(selection_changed)
        try:
            return await self._monitor_klipper_contents(interface, updated)
        except asyncio.CancelledError:
            raise
        except Exception:
            return False
        finally:
            interface.off_clipboard_history_updated(selection_changed)
            bus.disconnect()

    async def _monitor_klipper_contents(self, interface, updated: asyncio.Event) -> bool:
        # Initialize without publishing the clipboard contents that existed at startup.
        current = normalize_selection(await interface.call_get_clipboard_contents())
        self._observed_text = current

        while self._running:
            signaled = False
            try:
                await asyncio.wait_for(updated.wait(), timeout=0.2)
                signaled = True
                updated.clear()
            except TimeoutError:
                pass

            text = normalize_selection(await interface.call_get_clipboard_contents())
            if text != self._observed_text or signaled:
                self._schedule_candidate(
                    text,
                    SelectionSource.PRIMARY,
                    allow_duplicate=True,
                )

        return True

    def stop(self) -> None:
        self._running = False
        self._cancel_pending_publish()

    async def _publish(self, text: str, source: SelectionSource) -> None:
        snapshot = await self.context.snapshot()
        if not self.guard.accepts(text, snapshot):
            return

        await self.hub.publish(
            SelectionEvent(
                text=text,
                text_preview=preview(text),
                char_count=len(text),
                source=source,
                cursor_x=snapshot.cursor_x,
                cursor_y=snapshot.cursor_y,
                active_app=snapshot.active_resource_class,
            )
        )

    async def _watch_selection(self, source: SelectionSource) -> bool:
        args = ["wl-paste"]
        if source == SelectionSource.PRIMARY:
            args.append("--primary")
        args.extend(
            [
                "--type",
                "text",
                "--watch",
                "sh",
                "-c",
                self._watch_command(),
            ]
        )

        try:
            process = await asyncio.create_subprocess_exec(
                *args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL,
                limit=2 * 1024 * 1024,
            )
        except Exception:
            return False

        assert process.stdout is not None
        try:
            while self._running:
                try:
                    raw = await process.stdout.readuntil(b"\0")
                except (IncompleteReadError, LimitOverrunError):
                    return False

                text = normalize_selection(raw[:-1].decode("utf-8", errors="ignore"))
                self._schedule_candidate(text, source, allow_duplicate=True)

            return True
        finally:
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=0.4)
                except Exception:
                    process.kill()
                    await process.wait()

    def _schedule_candidate(
        self,
        text: str,
        source: SelectionSource,
        *,
        allow_duplicate: bool,
    ) -> None:
        self._cancel_pending_publish()

        if not text:
            self._last_text = ""
            self._observed_text = ""
            return

        if not allow_duplicate and text == self._last_text:
            return

        self._observed_text = text
        self._pending_publish = asyncio.create_task(self._debounced_publish(text, source))

    async def _debounced_publish(self, text: str, source: SelectionSource) -> None:
        await asyncio.sleep(self.debounce_seconds)
        if self._observed_text != text:
            return

        await self._publish(text, source)
        self._last_text = text

    def _cancel_pending_publish(self) -> None:
        if self._pending_publish and not self._pending_publish.done():
            self._pending_publish.cancel()
        self._pending_publish = None

    @staticmethod
    def _watch_command() -> str:
        # wl-paste --watch sends the current selection to this command on stdin.
        # Reading stdin avoids a second Wayland selection request while dragging.
        return "cat; printf '\\000'"

    @staticmethod
    def _is_kde_session() -> bool:
        desktop = os.getenv("XDG_CURRENT_DESKTOP", "").lower()
        return "kde" in desktop or bool(os.getenv("KDE_FULL_SESSION"))
