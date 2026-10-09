import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../integrations/kwin/openreader-context/contents/code/main.js", import.meta.url), "utf8");

function signal() {
  const listeners = [];
  return { connect: (fn) => listeners.push(fn), emit: (...args) => listeners.forEach((fn) => fn(...args)) };
}

function desktop(cursor = { x: 4000, y: 500 }) {
  const left = { geometry: { x: 0, y: 0, width: 2752, height: 1152 } };
  const right = { geometry: { x: 2752, y: 0, width: 2752, height: 1152 } };
  const pending = [];
  const workspace = {
    screens: [left, right], cursorPos: cursor, stackingOrder: [],
    cursorPosChanged: signal(), windowActivated: signal(),
    windowAdded: signal(), windowRemoved: signal(),
    sendClientToScreen(win, output) { win.output = output; },
    screenAt() { return null; },
  };
  vm.runInNewContext(source, {
    workspace,
    callDBus(...args) {
      if (args[3] === "Ping") pending.push(args.at(-1));
    },
  });
  const win = {
    caption: "OpenReader Overlay:66:66:1", output: left,
    frameGeometry: { x: 0, y: 0, width: 56, height: 56 },
    captionChanged: signal(),
  };
  workspace.windowAdded.emit(win);
  return { workspace, win, right, pending, flush() { while (pending.length) pending.shift()(); } };
}

test("missing/stale backend coordinates do not place the overlay on the left monitor", () => {
  const d = desktop();
  d.flush();
  assert.equal(d.win.frameGeometry.x, 4018);
  assert.equal(d.win.frameGeometry.y, 518);
  assert.equal(d.win.output, d.right);
});

test("overlay stays on the pointer monitor at an internal display boundary", () => {
  const d = desktop({ x: 2748, y: 1148 });
  d.flush();
  assert.equal(d.win.frameGeometry.x, 2696);
  assert.equal(d.win.frameGeometry.y, 1096);
  assert.equal(d.win.output, d.workspace.screens[0]);
});

test("negative logical monitor coordinates are preserved", () => {
  const d = desktop();
  d.workspace.screens[0].geometry.x = -2752;
  d.workspace.cursorPos = { x: -1000, y: 200 };
  d.win.caption = "OpenReader Overlay:66:66:2";
  d.win.captionChanged.emit();
  d.flush();
  assert.equal(d.win.frameGeometry.x, -982);
  assert.equal(d.win.frameGeometry.y, 218);
});

test("pointer movement after mapping does not move the click target", () => {
  const d = desktop();
  d.workspace.cursorPos = { x: 100, y: 100 };
  d.workspace.cursorPosChanged.emit();
  d.flush();
  assert.equal(d.win.frameGeometry.x, 4018);
  assert.equal(d.win.frameGeometry.y, 518);
});

test("old positioning replies cannot overwrite a newer selection", () => {
  const d = desktop();
  const oldReply = d.pending.shift();
  d.workspace.cursorPos = { x: 200, y: 300 };
  d.win.caption = "OpenReader Overlay:66:66:2";
  d.win.captionChanged.emit();
  d.flush();
  oldReply();
  assert.equal(d.win.frameGeometry.x, 218);
  assert.equal(d.win.frameGeometry.y, 318);
});

test("unmapped windows are not accessed by pending positioning replies", () => {
  const d = desktop();
  d.workspace.windowRemoved.emit(d.win);
  Object.defineProperty(d.win, "caption", { get() { throw new Error("deleted window"); } });
  assert.doesNotThrow(() => d.flush());
});
