"""Live KDE/Wayland check against a running OpenReader (system Python + GTK 3).

Opens a disposable entry, selects words repeatedly, and observes compositor and
GTK focus until the overlay times out. Does not inject global keyboard events.
Run: /usr/bin/python3 scripts/check-overlay-focus.py
"""

import json
from pathlib import Path
import subprocess

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402


def main() -> int:
    DBusGMainLoop(set_as_default=True)
    GLib.set_prgname("openreader-focus-check")
    reports = []
    focus_losses = []
    armed = False
    script_name = "openreader-focus-regression-check"

    class Reporter(dbus.service.Object):
        @dbus.service.method("org.openreader.FocusCheck", in_signature="s")
        def Report(self, payload):
            reports.append(json.loads(payload))

    bus_name = dbus.service.BusName("org.openreader.FocusCheck", bus=dbus.SessionBus())
    reporter = Reporter(bus_name, "/org/openreader/FocusCheck")
    window = Gtk.Window(title="OpenReader Focus Regression Check")
    window.set_default_size(480, 90)
    entry = Gtk.Entry()
    entry.set_text("Alpha Bravo Charlie Delta")
    window.add(entry)

    def focus_out(*_args):
        if armed:
            focus_losses.append(True)
        return False

    window.connect("focus-out-event", focus_out)
    destroy_handler = window.connect("destroy", Gtk.main_quit)
    window.show_all()
    entry.grab_focus()

    def qdbus(*args):
        return subprocess.check_output(["qdbus6", "org.kde.KWin", *args], text=True).strip()

    def load_probe():
        script_id = qdbus(
            "/Scripting", "org.kde.kwin.Scripting.loadScript",
            str(Path(__file__).with_suffix(".js").resolve()), script_name,
        )
        qdbus(f"/Scripting/Script{script_id}", "org.kde.kwin.Script.run")
        return False

    def select(start, end):
        nonlocal armed
        armed = True
        entry.select_region(start, end)
        return False

    GLib.timeout_add(300, load_probe)
    for delay, start, end in [(1200, 0, 5), (2200, 6, 11), (3200, 0, 5), (9000, 12, 19)]:
        GLib.timeout_add(delay, select, start, end)
    GLib.timeout_add(15000, lambda: (Gtk.main_quit(), False)[1])
    try:
        Gtk.main()
        overlays = [r for r in reports if r["overlay"]]
        result = {
            "overlay_observations": len(overlays),
            "overlay_sizes": sorted({(r["width"], r["height"]) for r in overlays}),
            "overlay_maps": sum(r["kind"] == "overlay-added" for r in overlays),
            "overlay_accepts_focus": any(r["wantsInput"] for r in overlays),
            "overlay_activations": sum(r["kind"] == "activation" for r in overlays),
            "overlay_matches_symbol_size": bool(overlays) and all(
                r["width"] == 56 and r["height"] == 56 for r in overlays
            ),
            "source_focus_losses": len(focus_losses),
            "source_still_focused": window.has_toplevel_focus(),
            "main_accepts_focus": any(r["kind"] == "main" and r["wantsInput"] for r in reports),
        }
        print(json.dumps(result, indent=2))
        return int(not (
            result["overlay_maps"] >= 2
            and not result["overlay_accepts_focus"]
            and not result["overlay_activations"]
            and result["overlay_matches_symbol_size"]
            and not result["source_focus_losses"]
            and result["source_still_focused"]
            and result["main_accepts_focus"]
        ))
    finally:
        qdbus("/Scripting", "org.kde.kwin.Scripting.unloadScript", script_name)
        reporter.remove_from_connection()
        window.disconnect(destroy_handler)
        window.destroy()


if __name__ == "__main__":
    raise SystemExit(main())
