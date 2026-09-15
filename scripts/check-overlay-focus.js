// Companion probe for check-overlay-focus.py. Only the test window is activated.
var sourceTitle = "OpenReader Focus Regression Check";

function report(kind, win) {
  callDBus(
    "org.openreader.FocusCheck", "/org/openreader/FocusCheck",
    "org.openreader.FocusCheck", "Report",
    JSON.stringify({
      kind: kind,
      sourceActive: workspace.activeWindow && workspace.activeWindow.caption === sourceTitle,
      wantsInput: win ? win.wantsInput : null,
      width: win ? win.frameGeometry.width : null,
      height: win ? win.frameGeometry.height : null,
      overlay: win ? String(win.caption).indexOf("OpenReader Overlay") === 0 : false
    })
  );
}

function observe(win) {
  var caption = String(win.caption);
  if (caption.indexOf("OpenReader Overlay") === 0) {
    report("overlay-added", win);
    win.captionChanged.connect(function () { report("overlay-title", win); });
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
