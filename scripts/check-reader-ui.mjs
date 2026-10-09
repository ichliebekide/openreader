// Runs against Vite, with a separate backend and silent test engine. No KWin hooks.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

const { chromium } = await import(process.env.OPENREADER_PLAYWRIGHT ?? "playwright");
const port = Number(process.env.OPENREADER_TEST_PORT ?? 8766);
const frontend = process.env.OPENREADER_TEST_UI ?? "http://127.0.0.1:1420";
const output = process.env.OPENREADER_TEST_OUTPUT ?? "/tmp/openreader-reader-ui";
await mkdir(output, { recursive: true });
const backend = spawn(resolve("backend/.venv/bin/python"), ["-c", `
from types import SimpleNamespace
import os
import re
import uvicorn
from openreader_backend import main
from openreader_backend.models import ReaderSettings, VoiceProfile, TTSEngine
from openreader_backend.tts.base import AudioChunk, WordBoundary
from openreader_backend.tts.orchestrator import TTSOrchestrator
profile = VoiceProfile(id="silent-ui-test", label="Silent test", engine=TTSEngine.EDGE)
settings = ReaderSettings(backend_engine=TTSEngine.EDGE, active_profile_id=profile.id, voice_profiles=[profile])
main.settings_store = SimpleNamespace(settings=settings)
main.tts = TTSOrchestrator(settings, main._word_progress)
class SilentEngine:
    async def synthesize(self, text, profile):
        words = tuple(WordBoundary(len(text[:m.start()].encode('utf-16-le')) // 2,
                                   len(text[:m.end()].encode('utf-16-le')) // 2,
                                   0.1 + i * 0.5, 0.5)
                      for i, m in enumerate(list(re.finditer(r'\\S+', text))[:15]))
        for i in range(80):
            yield AudioChunk(data=b"\\0\\0" * 2400, sample_rate=24000, encoding="raw_s16le",
                             words=words if i == 0 else ())
main.tts.engines[TTSEngine.EDGE] = SilentEngine()
@main.app.get("/__reader_ui_fixture")
async def fixture_identity():
    return {"pid": os.getpid()}
uvicorn.run(main.app, host="127.0.0.1", port=${port}, lifespan="off", log_level="warning")
`], { stdio: ["ignore", "pipe", "pipe"] });
let backendLog = "";
backend.stdout.on("data", data => { backendLog += data; });
backend.stderr.on("data", data => { backendLog += data; });
let browser;
let fixtureReady = false;
try {
  for (let attempt = 0; ; attempt++) {
    if (backend.exitCode !== null) throw new Error(backendLog);
    const ready = await fetch(`http://127.0.0.1:${port}/__reader_ui_fixture`).catch(() => null);
    if (ready?.ok && (await ready.json()).pid === backend.pid) {
      fixtureReady = true;
      break;
    }
    if (attempt > 100) throw new Error(`Test backend did not start: ${backendLog}`);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.OPENREADER_CHROMIUM || undefined
  });
  const page = await browser.newPage({ viewport: { width: 1240, height: 800 } });
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.route("http://127.0.0.1:8765/**", async route => {
    const response = await route.fetch({ url: route.request().url().replace(":8765", `:${port}`) });
    await route.fulfill({ response });
  });
  await page.routeWebSocket("ws://127.0.0.1:8765/**", ws => {
    ws.connectToServer(ws.url().replace(":8765", `:${port}`));
  });
  await page.goto(frontend);
  await page.getByRole("option", { name: "Silent test" }).waitFor({ state: "attached" });
  assert.deepEqual(await page.locator('select').first().locator('option').allTextContents(),
    ["Microsoft Seraphina", "Piper"]);
  await page.locator('.tts-status-row').nth(1).waitFor();
  assert.deepEqual(await page.locator('.tts-status-row > span:nth-child(2)').allTextContents(),
    ["Seraphina", "Piper"]);
  const text = "Erster Satz. Version 1.2.3 bleibt unveraendert! ".repeat(20).trim();
  await page.locator('input[type="file"]').setInputFiles({
    name: "test.txt", mimeType: "text/plain", buffer: Buffer.from(text)
  });
  await page.waitForFunction(expected => document.querySelector(".selection-box")?.textContent === expected, text);
  await page.getByRole("button", { name: "Vorlesen", exact: true }).click();
  await page.locator("mark.spoken-word").waitFor();
  assert(text.split(/\s+/).includes(await page.locator("mark.spoken-word").innerText()));
  await page.screenshot({ path: `${output}/word-highlight.png` });
  await page.getByRole("button", { name: "Pausieren", exact: true }).click();
  await page.getByRole("button", { name: "Fortsetzen", exact: true }).waitFor();
  assert.equal((await fetch(`http://127.0.0.1:${port}/api/tts/playback`).then(r => r.json())).paused, true);
  await page.waitForTimeout(150);
  const pausedWord = await page.locator("mark.spoken-word").textContent();
  await page.waitForTimeout(400);
  assert.equal(await page.locator("mark.spoken-word").textContent(), pausedWord);
  // A newly imported selection must not replace the text currently being spoken.
  await page.locator('input[type="file"]').setInputFiles({
    name: "next.txt", mimeType: "text/plain", buffer: Buffer.from("Andere Auswahl.")
  });
  await page.waitForTimeout(100);
  assert.equal(await page.locator(".selection-box").textContent(), text);
  await page.reload();
  await page.getByRole("button", { name: "Fortsetzen", exact: true }).waitFor();
  await page.locator("mark.spoken-word").waitFor();
  assert.equal(await page.locator("mark.spoken-word").textContent(), pausedWord);
  assert.equal(await page.locator(".selection-box").textContent(), text);
  await page.screenshot({ path: `${output}/desktop-paused.png` });
  await page.getByRole("button", { name: "Fortsetzen", exact: true }).click();
  await page.getByRole("button", { name: "Pausieren", exact: true }).waitFor();
  await page.waitForFunction(previous => document.querySelector("mark.spoken-word")?.textContent !== previous,
    pausedWord);
  await page.getByRole("button", { name: "Stoppen", exact: true }).click();
  await page.getByRole("button", { name: "Vorlesen", exact: true }).waitFor();
  assert.equal((await fetch(`http://127.0.0.1:${port}/api/tts/playback`).then(r => r.json())).speaking, false);
  assert.equal(await page.locator("mark.spoken-word").count(), 0);
  await page.locator('input[type="file"]').setInputFiles({
    name: "test.txt", mimeType: "text/plain", buffer: Buffer.from(text)
  });
  await page.waitForFunction(expected => document.querySelector(".selection-box")?.textContent === expected, text);
  await page.locator('input[type="file"]').setInputFiles({
    name: "broken.txt", mimeType: "text/plain", buffer: Buffer.from([0xff, 0x41])
  });
  await page.getByRole("alert").waitFor();
  assert.match(await page.getByRole("alert").innerText(), /UTF-8/);
  for (const [width, height] of [[1240, 800], [1040, 660], [390, 844]]) {
    await page.setViewportSize({ width, height });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    const controls = await page.locator(".action-row button").evaluateAll(buttons => buttons.map(button => {
      const rect = button.getBoundingClientRect();
      return { x: rect.x, right: rect.right, width: rect.width, height: rect.height };
    }));
    assert(controls.every(rect => rect.width >= 40 && rect.height >= 40 && rect.x >= 0 && rect.right <= width));
    assert.equal(await page.locator(".primary-panel").evaluate(panel => {
      const alert = panel.querySelector('[role="alert"]').getBoundingClientRect();
      const bounds = panel.getBoundingClientRect();
      return alert.bottom <= bounds.bottom && alert.top >= bounds.top;
    }), true);
    await page.screenshot({ path: `${output}/${width}x${height}.png` });
  }
  assert.deepEqual(errors, []);
  console.log(`PASS: timed word highlight, pause freeze, reading snapshot/reconnect, import, errors, stop, desktop/minimum/mobile layout; screenshots: ${output}`);
} finally {
  await browser?.close();
  if (fixtureReady && backend.exitCode === null) {
    await fetch(`http://127.0.0.1:${port}/api/tts/stop`, {
      method: "POST", signal: AbortSignal.timeout(2000)
    }).catch(() => {});
  }
  if (backend.exitCode === null) {
    const exited = once(backend, "exit");
    backend.kill("SIGTERM");
    const killTimer = setTimeout(() => backend.kill("SIGKILL"), 3000);
    await exited;
    clearTimeout(killTimer);
  }
}
