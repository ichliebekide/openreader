// Companion KWin probe for src-tauri/examples/check_overlay_focus.rs. Only the test window is activated.
var sourceTitle = "OpenReader Focus Regression Check";

function report(kind, win, expected) {
  callDBus(
    "org.openreader.FocusCheck", "/org/openreader/FocusCheck",
    "org.openreader.FocusCheck", "Report",
    JSON.stringify({
      kind: kind,
      sourceActive: workspace.activeWindow && workspace.activeWindow.caption === sourceTitle,
      wantsInput: win ? win.wantsInput : null,
      width: win ? win.frameGeometry.width : null,
      height: win ? win.frameGeometry.height : null,
      x: win ? win.frameGeometry.x : null,
      y: win ? win.frameGeometry.y : null,
      expected: expected || null,
      overlay: win ? String(win.caption).indexOf("OpenReader Overlay") === 0 : false
    })
  );
}

function checkPosition(win) {
  var pos = workspace.cursorPos;
  var screen = workspace.screenAt(pos).geometry;
  var expected = {
    x: Math.max(screen.x, Math.min(screen.x + screen.width - 56, Math.round(pos.x) + 18)),
    y: Math.max(screen.y, Math.min(screen.y + screen.height - 56, Math.round(pos.y) + 18))
  };
  callDBus("org.freedesktop.DBus", "/org/freedesktop/DBus",
    "org.freedesktop.DBus.Peer", "Ping", function () {
      report("overlay-position", win, expected);
    });
}

function observe(win) {
  var caption = String(win.caption);
  if (caption.indexOf("OpenReader Overlay") === 0) {
    report("overlay-added", win);
    checkPosition(win);
    win.captionChanged.connect(function () {
      report("overlay-title", win);
      checkPosition(win);
    });
  } else if (caption === "OpenReader") {
    report("main", win);
  }
}

workspace.windowActivated.connect(function (win) { report("activation", win); });
workspace.windowAdded.connect(observe);
var windows = workspace.stackingOrder;
for (var i = 0; i < windows.length; i++) {
  observe(windows[i]);
  if (String(windows[i].caption) === sourceTitle) {
    workspace.activeWindow = windows[i];
  }
}
report("ready", null);
