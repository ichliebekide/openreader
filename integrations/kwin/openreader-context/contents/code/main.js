var lastSentAt = 0;
var lastX = -1;
var lastY = -1;
var bridgeAvailable = false;
var bridgeCheckedAt = 0;
var bridgeCheckInFlight = false;
var contextReportInFlight = false;
var contextReportSentAt = 0;
var forcedReportPending = false;
var overlayWindows = [];

var sendIntervalMs = 120;
var bridgeCheckIntervalMs = 5000;
// KWin never calls the callback when a D-Bus call fails; treat a report
// without a reply after this long as lost instead of blocking all reports.
var reportReplyTimeoutMs = 1000;

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
  // Ignore replies for an unmapped window or a newer selection request.
  if (overlayWindows.indexOf(win) === -1 || text(win.caption) !== target.caption) {
    return;
  }
  var geometry = win.frameGeometry;
  var screen = target.output ? target.output.geometry : null;
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

  // The title signals a new request. Backend coordinates may be absent or stale
  // after a restart; only the compositor knows the current global pointer.
  var pos = workspace.cursorPos;
  if (!pos || !isFinite(pos.x) || !isFinite(pos.y)) {
    return;
  }
  var output = outputAt(pos.x, pos.y);
  if (!output) {
    return;
  }
  applyOverlayPosition(
    win,
    {
      x: Math.round(pos.x) + 18,
      y: Math.round(pos.y) + 18,
      output: output,
      caption: text(win.caption),
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
  if (contextReportInFlight && nowMs() - contextReportSentAt < reportReplyTimeoutMs) {
    // A window change must not be lost behind a pointer report in flight.
    if (force) {
      forcedReportPending = true;
    }
    return;
  }
  contextReportInFlight = false;

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
  contextReportSentAt = t;
  forcedReportPending = false;

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
        return;
      }
      if (forcedReportPending) {
        reportContext(true);
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

workspace.windowRemoved.connect(function (win) {
  var index = overlayWindows.indexOf(win);
  if (index !== -1) {
    overlayWindows.splice(index, 1);
  }
});

var existingWindows = workspace.stackingOrder || [];
for (var i = 0; i < existingWindows.length; i++) {
  configureOverlay(existingWindows[i]);
}

reportContext(true);
