//! Overlay positioning logic of the KWin script
//! (`integrations/kwin/openreader-context/contents/code/main.js`), run in the
//! boa JavaScript engine with a fake KWin workspace.
//!
//! Port of the former `scripts/check-overlay-position.test.mjs` (Node). The
//! script itself stays JavaScript because KWin only runs JavaScript.

use boa_engine::{Context, Source};

const SCRIPT: &str = include_str!("../../integrations/kwin/openreader-context/contents/code/main.js");

/// Fake KWin workspace/DBus, same as the Node harness.
const HARNESS: &str = r#"
function assertEqual(actual, expected, message) {
  if (actual !== expected) {
    throw new Error((message || "assertEqual") + ": expected " + expected + ", got " + actual);
  }
}

function signal() {
  var listeners = [];
  return {
    connect: function (fn) { listeners.push(fn); },
    emit: function () { var args = arguments; listeners.forEach(function (fn) { fn.apply(null, args); }); }
  };
}

function desktop(cursor) {
  cursor = cursor || { x: 4000, y: 500 };
  var left = { geometry: { x: 0, y: 0, width: 2752, height: 1152 } };
  var right = { geometry: { x: 2752, y: 0, width: 2752, height: 1152 } };
  var pending = [];
  var workspace = {
    screens: [left, right], cursorPos: cursor, stackingOrder: [],
    cursorPosChanged: signal(), windowActivated: signal(),
    windowAdded: signal(), windowRemoved: signal(),
    sendClientToScreen: function (win, output) { win.output = output; },
    screenAt: function () { return null; }
  };
  var callDBus = function () {
    if (arguments[3] === "Ping") pending.push(arguments[arguments.length - 1]);
  };
  loadKWinScript(workspace, callDBus);
  var win = {
    caption: "OpenReader Overlay:66:66:1", output: left,
    frameGeometry: { x: 0, y: 0, width: 72, height: 72 },
    captionChanged: signal()
  };
  workspace.windowAdded.emit(win);
  return {
    workspace: workspace, win: win, right: right, pending: pending,
    flush: function () { while (pending.length) pending.shift()(); }
  };
}
"#;

fn run(test: &str) {
    let mut context = Context::default();
    // Each desktop() gets a fresh copy of the script's globals, like Node's vm context.
    let loader = format!("function loadKWinScript(workspace, callDBus) {{\n{SCRIPT}\n}}\n{HARNESS}");
    context
        .eval(Source::from_bytes(&loader))
        .unwrap_or_else(|error| panic!("loading the KWin script failed: {error}"));
    if let Err(error) = context.eval(Source::from_bytes(&format!("(function () {{\n{test}\n}})();"))) {
        panic!("{error}");
    }
}

#[test]
fn missing_or_stale_backend_coordinates_do_not_place_the_overlay_on_the_left_monitor() {
    run(r#"
      var d = desktop();
      d.flush();
      assertEqual(d.win.frameGeometry.x, 4018, "x");
      assertEqual(d.win.frameGeometry.y, 518, "y");
      assertEqual(d.win.output, d.right, "output");
    "#);
}

#[test]
fn overlay_stays_on_the_pointer_monitor_at_an_internal_display_boundary() {
    run(r#"
      var d = desktop({ x: 2748, y: 1148 });
      d.flush();
      assertEqual(d.win.frameGeometry.x, 2680, "x");
      assertEqual(d.win.frameGeometry.y, 1080, "y");
      assertEqual(d.win.output, d.workspace.screens[0], "output");
    "#);
}

#[test]
fn negative_logical_monitor_coordinates_are_preserved() {
    run(r#"
      var d = desktop();
      d.workspace.screens[0].geometry.x = -2752;
      d.workspace.cursorPos = { x: -1000, y: 200 };
      d.win.caption = "OpenReader Overlay:66:66:2";
      d.win.captionChanged.emit();
      d.flush();
      assertEqual(d.win.frameGeometry.x, -982, "x");
      assertEqual(d.win.frameGeometry.y, 218, "y");
    "#);
}

#[test]
fn pointer_movement_after_mapping_does_not_move_the_click_target() {
    run(r#"
      var d = desktop();
      d.workspace.cursorPos = { x: 100, y: 100 };
      d.workspace.cursorPosChanged.emit();
      d.flush();
      assertEqual(d.win.frameGeometry.x, 4018, "x");
      assertEqual(d.win.frameGeometry.y, 518, "y");
    "#);
}

#[test]
fn old_positioning_replies_cannot_overwrite_a_newer_selection() {
    run(r#"
      var d = desktop();
      var oldReply = d.pending.shift();
      d.workspace.cursorPos = { x: 200, y: 300 };
      d.win.caption = "OpenReader Overlay:66:66:2";
      d.win.captionChanged.emit();
      d.flush();
      oldReply();
      assertEqual(d.win.frameGeometry.x, 218, "x");
      assertEqual(d.win.frameGeometry.y, 318, "y");
    "#);
}

#[test]
fn unmapped_windows_are_not_accessed_by_pending_positioning_replies() {
    run(r#"
      var d = desktop();
      d.workspace.windowRemoved.emit(d.win);
      Object.defineProperty(d.win, "caption", { get: function () { throw new Error("deleted window"); } });
      d.flush();
    "#);
}

#[test]
#[should_panic(expected = "expected 2, got 1")]
fn harness_reports_failed_assertions() {
    run("assertEqual(1, 2, 'sanity');");
}

#[test]
fn non_overlay_windows_and_malformed_titles_are_ignored() {
    run(r#"
      var d = desktop();
      d.flush();
      var other = { caption: "Kate", frameGeometry: { x: 5, y: 6, width: 300, height: 200 }, captionChanged: signal() };
      d.workspace.windowAdded.emit(other);
      d.flush();
      assertEqual(other.frameGeometry.x, 5, "other window untouched");
      d.win.caption = "OpenReader Overlay:abc";
      d.workspace.cursorPos = { x: 10, y: 10 };
      d.win.captionChanged.emit();
      d.flush();
      assertEqual(d.win.frameGeometry.x, 4018, "malformed title keeps the old position");
    "#);
}

/// Context reports: a window change during a pointer report in flight is sent
/// afterwards, and a report whose reply never comes does not block later ones.
const CONTEXT_HARNESS: &str = r#"
function contextDesktop() {
  var calls = [];
  var workspace = {
    screens: [], cursorPos: { x: 10, y: 10 }, stackingOrder: [],
    activeWindow: { resourceClass: "org.kde.konsole", resourceName: "konsole" },
    cursorPosChanged: signal(), windowActivated: signal(),
    windowAdded: signal(), windowRemoved: signal(),
    sendClientToScreen: function () {}, screenAt: function () { return null; }
  };
  var callDBus = function () {
    var args = Array.prototype.slice.call(arguments);
    calls.push({ method: args[3], args: args.slice(4, -1), reply: args[args.length - 1] });
  };
  loadKWinScript(workspace, callDBus);
  return {
    workspace: workspace,
    reports: function () { return calls.filter(function (c) { return c.method === "ReportContext"; }); },
    answer: function (method, value) {
      for (var i = 0; i < calls.length; i++) {
        if (calls[i].method === method && !calls[i].answered) {
          calls[i].answered = true;
          calls[i].reply(value);
          return;
        }
      }
      throw new Error("no pending " + method);
    }
  };
}
"#;

#[test]
fn window_change_during_a_report_in_flight_is_not_lost() {
    run(&format!(
        r#"{CONTEXT_HARNESS}
      var d = contextDesktop();
      d.answer("NameHasOwner", true);
      assertEqual(d.reports().length, 1, "initial report");
      d.workspace.activeWindow = {{ resourceClass: "firefox", resourceName: "firefox" }};
      d.workspace.windowActivated.emit();
      assertEqual(d.reports().length, 1, "waits for the reply");
      d.answer("ReportContext", true);
      assertEqual(d.reports().length, 2, "sent after the reply");
      assertEqual(d.reports()[1].args[2], "firefox", "new active window");
    "#
    ));
}

#[test]
fn a_report_without_reply_expires() {
    run(&format!(
        r#"{CONTEXT_HARNESS}
      var d = contextDesktop();
      d.answer("NameHasOwner", true);
      assertEqual(d.reports().length, 1, "initial report");
      var realNow = Date.now;
      Date.now = function () {{ return realNow() + 1500; }};
      d.workspace.activeWindow = {{ resourceClass: "firefox", resourceName: "firefox" }};
      d.workspace.windowActivated.emit();
      Date.now = realNow;
      assertEqual(d.reports().length, 2, "lost reply no longer blocks");
    "#
    ));
}
