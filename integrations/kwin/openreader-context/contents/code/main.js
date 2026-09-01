var lastSentAt = 0;
var lastX = -1;
var lastY = -1;
var bridgeAvailable = false;
var bridgeCheckedAt = 0;
var bridgeCheckInFlight = false;
var contextReportInFlight = false;
var overlayWindows = [];

var sendIntervalMs = 120;
var bridgeCheckIntervalMs = 5000;

function nowMs() {
  return Date.now ? Date.now() : new Date().getTime();
}

function currentWindowClass() {
  var win = workspace.activeWindow;
  if (!win) {
    return ["", ""];
  }
  return [String(win.resourceClass || ""), String(win.resourceName || "")];
}

function text(value) {
  return String(value || "");
}

function isOverlayWindow(win) {
  return win && text(win.caption).indexOf("OpenReader Overlay") === 0;
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function outputAt(x, y) {
  var outputs = workspace.screens || [];
  for (var i = 0; i < outputs.length; i++) {
    var geometry = outputs[i].geometry;
    if (
      geometry &&
      x >= geometry.x &&
      x < geometry.x + geometry.width &&
      y >= geometry.y &&
      y < geometry.y + geometry.height
    ) {
      return outputs[i];
    }
  }
  return workspace.screenAt(workspace.cursorPos);
}

function applyOverlayPosition(win, target, attempt) {
  var geometry = win.frameGeometry;
  var screen = workspace.virtualScreenGeometry;
  if (target.output && win.output !== target.output) {
    workspace.sendClientToScreen(win, target.output);
  }
  var x = target.x;
  var y = target.y;
  if (screen) {
    x = clamp(x, screen.x, screen.x + screen.width - geometry.width);
    y = clamp(y, screen.y, screen.y + screen.height - geometry.height);
  }

  win.frameGeometry = {
    x: x,
    y: y,
    width: geometry.width,
    height: geometry.height,
  };

  if (attempt < 2) {
    callDBus(
      "org.freedesktop.DBus",
      "/org/freedesktop/DBus",
      "org.freedesktop.DBus.Peer",
      "Ping",
      function () {
        applyOverlayPosition(win, target, attempt + 1);
      }
    );
  }
}

function positionOverlay(win) {
  var match = text(win.caption).match(/^OpenReader Overlay:(-?\d+):(-?\d+):\d+$/);
  if (!match) {
    return;
  }

  var x = Number(match[1]);
  var y = Number(match[2]);
  applyOverlayPosition(
    win,
    {
      x: x,
      y: y,
      output: outputAt(x, y),
    },
    0
  );
}

function configureOverlay(win) {
  if (!isOverlayWindow(win) || overlayWindows.indexOf(win) !== -1) {
    return;
  }

  overlayWindows.push(win);
  win.keepAbove = true;
  win.skipTaskbar = true;
  win.skipPager = true;
  win.skipSwitcher = true;
  win.captionChanged.connect(function () {
    positionOverlay(win);
  });
  positionOverlay(win);
}

function refreshBridgeOwner() {
  var t = nowMs();
  if (bridgeCheckInFlight || t - bridgeCheckedAt < bridgeCheckIntervalMs) {
    return;
  }

  bridgeCheckInFlight = true;
  bridgeCheckedAt = t;
  callDBus(
    "org.freedesktop.DBus",
    "/org/freedesktop/DBus",
    "org.freedesktop.DBus",
    "NameHasOwner",
    "org.openreader.Desktop",
    function (owned) {
      bridgeAvailable = owned === true || String(owned) === "true";
      bridgeCheckInFlight = false;
      contextReportInFlight = false;
      if (bridgeAvailable) {
        reportContext(true);
      }
    }
  );
}

function reportContext(force) {
  refreshBridgeOwner();
  if (!bridgeAvailable) {
    return;
  }
  if (contextReportInFlight) {
    return;
  }

  var pos = workspace.cursorPos;
  if (!pos) {
    return;
  }

  var t = nowMs();
  var x = Math.round(pos.x);
  var y = Math.round(pos.y);
  if (!force && t - lastSentAt < sendIntervalMs) {
    return;
  }
  if (!force && x === lastX && y === lastY) {
    return;
  }

  lastSentAt = t;
  lastX = x;
  lastY = y;
  contextReportInFlight = true;

  var active = currentWindowClass();
  callDBus(
    "org.openreader.Desktop",
    "/org/openreader/Desktop",
    "org.openreader.Desktop",
    "ReportContext",
    x,
    y,
    active[0],
    active[1],
    function (accepted) {
      contextReportInFlight = false;
      if (!(accepted === true || String(accepted) === "true")) {
        bridgeAvailable = false;
        bridgeCheckedAt = 0;
      }
    }
  );
}

workspace.cursorPosChanged.connect(function () {
  reportContext(false);
});

workspace.windowActivated.connect(function () {
  reportContext(true);
});

workspace.windowAdded.connect(function (win) {
  configureOverlay(win);
});

var existingWindows = workspace.stackingOrder || [];
for (var i = 0; i < existingWindows.length; i++) {
  configureOverlay(existingWindows[i]);
}

reportContext(true);
