import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { chromium } from "playwright";

const desktop = fileURLToPath(new URL("../", import.meta.url));
const viteEntry = join(desktop, "node_modules", "vite", "bin", "vite.js");
const baseUrl = process.env.LINKVAULT_EDITOR_EVALUATION_URL
  ?? "http://127.0.0.1:1421/editor-evaluation.html";
const ownsServer = !process.env.LINKVAULT_EDITOR_EVALUATION_URL;
const actionTimeoutMs = Number.parseInt(process.env.LINKVAULT_EDITOR_EVALUATION_ACTION_TIMEOUT_MS ?? "15000", 10);
const checkTimeoutMs = Number.parseInt(process.env.LINKVAULT_EDITOR_EVALUATION_CHECK_TIMEOUT_MS ?? "30000", 10);
const navigationTimeoutMs = Number.parseInt(process.env.LINKVAULT_EDITOR_EVALUATION_NAVIGATION_TIMEOUT_MS ?? "30000", 10);
const checks = [];
const consoleErrors = [];
const pageErrors = [];
const externalRequests = [];
const failedResponses = [];
let vite;
let browser;
let activeCheckId;
let matrixAbort;

async function assertPortFree() {
  const probe = createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(1421, "127.0.0.1", resolve);
  });
  await new Promise((resolve, reject) => probe.close((error) => error ? reject(error) : resolve()));
}

async function waitForServer() {
  const deadline = Date.now() + 30_000;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(baseUrl);
      if (response.ok) return;
      lastError = new Error(`received ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Vite did not serve ${baseUrl}: ${lastError ?? "timed out"}`);
}

async function stopOwnedVite() {
  if (!vite || vite.exitCode !== null) return;
  const closed = once(vite, "close");
  vite.kill("SIGTERM");
  await Promise.race([
    closed,
    new Promise((resolve) => setTimeout(resolve, 5_000))
  ]);
}

async function check(id, run) {
  activeCheckId = id;
  const startedAt = Date.now();
  let timeoutId;
  const timeout = new Promise((_, reject) => {
    timeoutId = setTimeout(() => {
      reject(new Error(`Timed out after ${checkTimeoutMs}ms`));
    }, checkTimeoutMs);
  });
  console.log(`CHECK_START=${id}`);
  try {
    const runPromise = Promise.resolve().then(run);
    // A timed-out browser protocol operation can reject after its containing
    // check has stopped. Keep that rejection observed while the runner closes.
    runPromise.catch(() => {});
    const details = await Promise.race([runPromise, timeout]);
    checks.push({ id, status: "pass", details, elapsedMs: Date.now() - startedAt });
  } catch (error) {
    const details = String(error?.stack ?? error);
    checks.push({ id, status: "fail", details, elapsedMs: Date.now() - startedAt });
    if (details.includes(`Timed out after ${checkTimeoutMs}ms`)) {
      matrixAbort = { id, details };
      throw error;
    }
  } finally {
    clearTimeout(timeoutId);
    console.log(`CHECK_END=${id}; ELAPSED_MS=${Date.now() - startedAt}`);
    if (!matrixAbort) activeCheckId = undefined;
  }
}

async function captureMarkdown(page) {
  const markdown = await page.evaluate(() => window.__CLIPPING_EDITOR_EVALUATION__?.captureMarkdown() ?? "");
  assert.equal(typeof markdown, "string", "the adapter must return serialized Markdown as a string");
  return markdown;
}

async function activeEditor(page) {
  const editor = page.locator('[data-editor-root="true"] [contenteditable="true"]');
  await editor.waitFor({ state: "visible", timeout: 8_000 });
  return editor;
}

async function loadFixture(page, buttonName, expectedText) {
  await page.getByRole("button", { name: buttonName, exact: true }).click();
  if (expectedText) {
    await page.locator('[data-editor-root="true"]').getByText(expectedText, { exact: true }).waitFor();
  }
}

function containsMdxEsmDirective(markdown) {
  const lines = markdown.split("\n");
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const trimmed = lines[lineIndex].trimStart();
    if (/^export\s+(?:default\b|\{|\*)/.test(trimmed)) return true;
    if (/^export\s+(?:(?:async\s+)?(?:class|function)|const|enum|interface|let|type|var)\b/.test(trimmed)) {
      for (let candidateIndex = lineIndex; candidateIndex < Math.min(lines.length, lineIndex + 256); candidateIndex += 1) {
        const candidate = lines[candidateIndex];
        if (candidateIndex > lineIndex && candidate.trim() === "") break;
        if (/[=;({]/.test(candidate)) return true;
      }
    }
    if (!/^import\b/.test(trimmed)) continue;
    for (let candidateIndex = lineIndex; candidateIndex < Math.min(lines.length, lineIndex + 256); candidateIndex += 1) {
      const candidate = lines[candidateIndex];
      if (candidateIndex > lineIndex && candidate.trim() === "") break;
      if (/\bfrom\s+["'][^"']+["']\s*;?\s*$/.test(candidate)) return true;
      if (candidateIndex === lineIndex && /^import\s*["'][^"']+["']\s*;?\s*$/.test(candidate.trim())) return true;
    }
  }
  return false;
}

function assertNoExecutableMarkdown(markdown) {
  for (const tag of ["script", "img", "iframe", "video", "audio", "unsafe"]) {
    assert.doesNotMatch(markdown, new RegExp(`(^|[^\\\\])<${tag}\\b`, "i"), `serialized Markdown retained raw <${tag}>`);
  }
  assert.equal(containsMdxEsmDirective(markdown), false, "serialized Markdown retained an MDX ESM directive");
  assert.doesNotMatch(markdown, /[{}]/, "serialized Markdown retained an MDX expression delimiter");
  assert.doesNotMatch(markdown, /(^|[^\\])`[^`]+`/, "serialized Markdown retained inline code syntax");
  assert.doesNotMatch(markdown, /(^|\n)```/, "serialized Markdown retained a fenced code block");
  assert.doesNotMatch(markdown, /^\s*\|.*\|\s*$/m, "serialized Markdown retained a pipe-wrapped GFM table line");
  assert.doesNotMatch(markdown, /(^|\n)\s*- \[[ xX]\] /, "serialized Markdown retained a task list item");
  assert.doesNotMatch(markdown, /\[\^[^\]]+\]/, "serialized Markdown retained a footnote");
  assert.doesNotMatch(markdown, /!\[[^\]]*\]\(/, "serialized Markdown retained an image");
  assert.doesNotMatch(markdown, /\]\((?:javascript|data|vbscript|file):/i, "serialized Markdown retained an unsafe link");
}

try {
  if (ownsServer) {
    await assertPortFree();
    vite = spawn(process.execPath, [viteEntry, "--config", "vite.editor-evaluation.config.ts"], {
      cwd: desktop,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"]
    });
    vite.stdout.on("data", (chunk) => process.stdout.write(chunk));
    vite.stderr.on("data", (chunk) => process.stderr.write(chunk));
    console.log(`ISOLATED_VITE_PID=${vite.pid}; ENTRY=${viteEntry}; PORT=1421`);
    await waitForServer();
  }

  browser = await chromium.launch({
    channel: process.env.PLAYWRIGHT_CHANNEL || "chrome",
    headless: true
  });
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const page = await context.newPage();
  page.setDefaultTimeout(actionTimeoutMs);
  const localOrigin = new URL(baseUrl).origin;

  await context.route("**/*", async (route) => {
    const url = route.request().url();
    if (url.startsWith("data:") || url.startsWith(localOrigin)) {
      await route.continue();
      return;
    }
    externalRequests.push(url);
    await route.abort();
  });
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => pageErrors.push(String(error)));
  page.on("response", (response) => {
    if (response.url().startsWith(localOrigin) && response.status() >= 400) {
      failedResponses.push(`${response.status()} ${response.url()}`);
    }
  });

  await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: navigationTimeoutMs });
  await page.locator('[data-editor-root="true"]').waitFor({ state: "visible" });

  await check("adapter root, React 19 Strict Mode lifecycle, and accessible body", async () => {
    assert.equal(await page.locator('[data-editor-root="true"]').count(), 1, "expected exactly one adapter root");
    assert.equal(await page.getByLabel("Clipping note editor", { exact: true }).count(), 1, "adapter root must have its required accessible name");
    assert.equal(await page.getByLabel("Clipping note editor body", { exact: true }).count(), 1, "editable body must have an accessible name");
    assert.equal(await page.locator('[data-editor-root="true"] [contenteditable="true"]').count(), 1, "expected one editable surface");
    const readyCount = await page.evaluate(() => window.__CLIPPING_EDITOR_EVALUATION__?.readyCount ?? -1);
    assert.equal(readyCount, 1, "React Strict Mode must create one committed editor and ready callback");
    return { readyCount };
  });

  await check("common Markdown fixture renders every approved construct", async () => {
    const root = page.locator('[data-editor-root="true"]');
    for (const text of ["Research note", "bold", "italic", "removed", "First point", "Nested item", "One", "Two", "Quoted observation", "Source"]) {
      assert.ok(await root.getByText(text, { exact: true }).count(), `missing semantic fixture text: ${text}`);
    }
    assert.equal(await root.locator("h1").count(), 1);
    assert.ok(await root.locator("strong").count() >= 1, "bold markup did not render semantically");
    assert.ok(await root.locator("em").count() >= 1, "italic markup did not render semantically");
    assert.ok(await root.locator("s, del").count() >= 1, "strikethrough markup did not render semantically");
    assert.ok(await root.locator("ul ul").count() >= 1, "nested list did not render");
    assert.equal(await root.locator("ol").count(), 1);
    assert.equal(await root.locator("blockquote").count(), 1);
    assert.equal(await root.locator('a[href="https://example.com/path?q=test"]').count(), 1);
    return "approved constructs rendered";
  });

  await check("common fixture serializes, reloads, and retains semantic structure", async () => {
    const serialized = await captureMarkdown(page);
    for (const fragment of ["# Research note", "**bold**", "*italic*", "~~removed~~", "Nested item", "Quoted observation", "https://example.com/path?q=test"]) {
      assert.ok(serialized.includes(fragment), `serialized Markdown lost ${fragment}`);
    }
    await page.getByRole("button", { name: "Reload captured Markdown", exact: true }).click();
    const root = page.locator('[data-editor-root="true"]');
    await root.getByText("Research note", { exact: true }).waitFor();
    assert.equal(await root.locator("h1").count(), 1);
    assert.ok(await root.locator("ul ul").count() >= 1);
    assert.equal(await root.locator("blockquote").count(), 1);
    return { serializedLength: serialized.length };
  });

  await check("headings one through four and hard breaks round trip", async () => {
    await loadFixture(page, "Load heading fixture", "Heading one");
    const root = page.locator('[data-editor-root="true"]');
    for (const [tag, text] of [["h1", "Heading one"], ["h2", "Heading two"], ["h3", "Heading three"], ["h4", "Heading four"]]) {
      assert.equal(await root.locator(tag).getByText(text, { exact: true }).count(), 1, `${tag} did not render`);
    }
    assert.equal(await root.locator("br").count(), 1, "Markdown hard break did not render as one break");
    const serialized = await captureMarkdown(page);
    for (const fragment of ["# Heading one", "## Heading two", "### Heading three", "#### Heading four", "Paragraph with a hard break."]) {
      assert.ok(serialized.includes(fragment), `serialized heading fixture lost ${fragment}`);
    }
    assert.match(serialized, /Paragraph with a hard break\.(?: {2}|\\)\nSecond line\./, "hard break did not serialize as Markdown");
    await page.getByRole("button", { name: "Reload captured Markdown", exact: true }).click();
    assert.equal(await page.locator('[data-editor-root="true"] h4').count(), 1);
    assert.equal(await page.locator('[data-editor-root="true"] br').count(), 1, "hard break did not survive reload");
    return { serializedLength: serialized.length };
  });

  await check("empty document and 2 MiB boundary fixtures serialize safely", async () => {
    await loadFixture(page, "Load empty document");
    assert.equal(await captureMarkdown(page), "");
    await page.getByRole("button", { name: "Load 2 MiB fixture", exact: true }).click();
    const serialized = await captureMarkdown(page);
    assert.ok(serialized.length >= (2 * 1024 * 1024) - 256, `expected about 2 MiB, received ${serialized.length}`);
    return { serializedLength: serialized.length };
  });

  await check("typing, formatting, list, Chinese committed text, undo, and redo survive", async () => {
    await loadFixture(page, "Load empty document");
    let editor = await activeEditor(page);
    await editor.click();
    const beforeChanges = await page.evaluate(() => window.__CLIPPING_EDITOR_EVALUATION__?.changeCount ?? -1);
    await page.keyboard.insertText("Browser transaction marker");
    await page.waitForTimeout(60);
    assert.ok((await captureMarkdown(page)).includes("Browser transaction marker"), "typed text did not serialize");
    const afterChanges = await page.evaluate(() => window.__CLIPPING_EDITOR_EVALUATION__?.changeCount ?? -1);
    assert.equal(afterChanges - beforeChanges, 1, "one typing transaction must not emit duplicate changes");
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(60);
    assert.ok(!(await captureMarkdown(page)).includes("Browser transaction marker"), "undo did not remove typed text");
    await page.keyboard.press("Control+y");
    await page.waitForTimeout(60);
    assert.ok((await captureMarkdown(page)).includes("Browser transaction marker"), "redo did not restore typed text");

    await page.keyboard.press("Control+a");
    await page.getByRole("button", { name: "Bold", exact: true }).click();
    assert.ok((await captureMarkdown(page)).includes("**Browser transaction marker**"), "toolbar formatting did not serialize as Markdown");
    await page.getByRole("button", { name: "Bulleted list", exact: true }).click();
    assert.match(await captureMarkdown(page), /- \*\*Browser transaction marker\*\*/, "toolbar list did not serialize as Markdown");

    await loadFixture(page, "Load empty document");
    editor = await activeEditor(page);
    await editor.click();
    await page.keyboard.insertText("世界日報剪報測試。");
    assert.ok((await captureMarkdown(page)).includes("世界日報剪報測試。"), "committed Chinese text did not serialize");
    await page.keyboard.press("Control+z");
    assert.ok(!(await captureMarkdown(page)).includes("世界日報剪報測試。"), "Chinese committed text did not undo");
    await page.keyboard.press("Control+y");
    assert.ok((await captureMarkdown(page)).includes("世界日報剪報測試。"), "Chinese committed text did not redo");
    return "committed input/history behavior passed; this is not native IME proof";
  });

  await check("parent acknowledgement, no-op rerender, and failed acknowledgement preserve content", async () => {
    const before = await captureMarkdown(page);
    await page.getByRole("button", { name: "Simulate parent acknowledgement", exact: true }).click();
    await page.getByRole("button", { name: "Simulate no-op parent rerender", exact: true }).click();
    await page.getByRole("button", { name: "Simulate failed parent acknowledgement", exact: true }).click();
    await page.waitForTimeout(40);
    assert.equal(await captureMarkdown(page), before, "controlled parent rerender reset active editor content");
    return "all parent update forms left editor content intact";
  });

  await check("document switch isolates content and undo history", async () => {
    await loadFixture(page, "Load common fixture", "Research note");
    const editor = await activeEditor(page);
    await editor.click();
    await page.keyboard.press("Control+End");
    await page.keyboard.insertText(" Old document history marker");
    await page.getByRole("button", { name: "Switch to second clipping", exact: true }).click();
    const second = await captureMarkdown(page);
    assert.ok(second.includes("Separate clipping"));
    assert.ok(!second.includes("Old document history marker"), "old document content leaked into second clipping");
    const secondEditor = await activeEditor(page);
    await secondEditor.click();
    await page.keyboard.press("Control+z");
    assert.ok(!(await captureMarkdown(page)).includes("Old document history marker"), "old document undo history crossed the document boundary");
    return "content and history are isolated";
  });

  await check("synthetic composition guard does not cross documents or duplicate a change", async () => {
    await loadFixture(page, "Load empty document");
    let root = page.locator('[data-editor-root="true"]');
    const editor = await activeEditor(page);
    const before = await page.evaluate(() => window.__CLIPPING_EDITOR_EVALUATION__?.changeCount ?? -1);
    await root.dispatchEvent("compositionstart");
    await editor.click();
    await page.keyboard.insertText("文");
    await root.dispatchEvent("compositionend");
    await page.waitForTimeout(60);
    const after = await page.evaluate(() => window.__CLIPPING_EDITOR_EVALUATION__?.changeCount ?? -1);
    assert.ok(after - before >= 0 && after - before <= 1, `composition emitted ${after - before} change events`);
    await root.dispatchEvent("compositionstart");
    await page.keyboard.insertText("中");
    await page.getByRole("button", { name: "Switch to second clipping", exact: true }).click();
    root = page.locator('[data-editor-root="true"]');
    await root.dispatchEvent("compositionend");
    const second = await captureMarkdown(page);
    assert.ok(second.includes("Separate clipping"));
    assert.ok(!second.includes("中"), "composition text from the old document leaked after switch");
    return "synthetic composition only; native IME remains unverified";
  });

  await check("unsupported input is inert, subset-only, and unsafe links cannot open", async () => {
    await page.getByRole("button", { name: "Load adversarial fixture", exact: true }).click();
    await page.waitForTimeout(100);
    const root = page.locator('[data-editor-root="true"]');
    const serialized = await captureMarkdown(page);
    assert.equal(await page.evaluate(() => window.__editor_executed), undefined, "HTML or MDX expression executed");
    assert.equal(await root.locator("img, table, pre, code, input[type=checkbox], iframe, video, audio").count(), 0, "unsupported content rendered semantically");
    const unsafeHrefs = await root.locator("a").evaluateAll((links) => links
      .map((link) => link.getAttribute("href") ?? "")
      .filter((href) => /^(?:javascript|data|vbscript|file):/i.test(href))
    );
    assert.deepEqual(unsafeHrefs, [], `unsafe links stayed interactive: ${unsafeHrefs.join(", ")}`);
    assertNoExecutableMarkdown(serialized);
    const safeLink = root.locator('a[href="https://example.com"]');
    assert.equal(await safeLink.count(), 1, "safe https link was not retained");
    const beforeUrl = page.url();
    await safeLink.click();
    await page.waitForTimeout(40);
    assert.equal(page.url(), beforeUrl, "editor link click attempted navigation/opening");
    return { serializedLength: serialized.length };
  });

  await check("nested MDX, multiline ESM, and pipe-less GFM tables cannot round trip", async () => {
    await loadFixture(page, "Load MDX edge-case fixture", "Before (foo: (bar: 1)) after");
    const root = page.locator('[data-editor-root="true"]');
    const serialized = await captureMarkdown(page);
    assertNoExecutableMarkdown(serialized);
    for (const fragment of ["{", "}", "import {", "thing", "export const x =", "value:1", "a | b", "--- | ---", "1 | 2", "c | d", "- | -", "3 | 4"]) {
      assert.ok(!serialized.includes(fragment), `unsupported source fragment survived serialization: ${fragment}`);
    }
    for (const retainedProse of ["Before import", "After import", "Before export", "After export", "Before table", "After table", "Before short table", "After short table", "import findings from yesterday", "export const findings from yesterday"]) {
      assert.ok(serialized.includes(retainedProse), `safe surrounding prose was lost: ${retainedProse}`);
    }
    assert.equal(
      await root.locator('a[href="https://example.com/explicit"]').count(),
      1,
      "safe explicit link was removed while scanning nearby prose"
    );
    assert.equal(await root.locator("table").count(), 0, "pipe-less GFM input rendered as a table");
    return { serializedLength: serialized.length };
  });

  await check("rich HTML is flattened and image/file paste is rejected with the required copy", async () => {
    await loadFixture(page, "Load empty document");
    const editor = await activeEditor(page);
    await editor.evaluate((element) => {
      const transfer = new DataTransfer();
      transfer.setData("text/html", "<strong>Rich</strong><img src=\"https://example.invalid/paste.png\">");
      transfer.setData("text/plain", "Flattened pasted note");
      element.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: transfer }));
    });
    const flattenedMarkdown = await captureMarkdown(page);
    assert.ok(flattenedMarkdown.includes("Flattened pasted note"), "rich HTML paste did not retain its plain-text representation");
    assert.doesNotMatch(flattenedMarkdown, /<strong|<img|!\[/i, "rich HTML paste retained unsupported markup");
    assert.equal(await page.locator('[data-editor-root="true"] strong, [data-editor-root="true"] img').count(), 0, "rich HTML paste rendered unsupported DOM");

    await editor.evaluate((element) => {
      const transfer = new DataTransfer();
      transfer.items.add(new File(["fixture"], "pasted.png", { type: "image/png" }));
      element.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: transfer }));
    });
    await page.getByText("Images aren't supported inside clipping notes.", { exact: true }).waitFor();
    return "rich HTML was flattened and image/file paste was rejected non-destructively";
  });

  await check("toolbar order, keyboard focus, dialog trap, labels, and pressed states are accessible", async () => {
    await loadFixture(page, "Load common fixture", "Research note");
    const toolbar = page.locator('[data-editor-root="true"] [role="toolbar"]');
    const controls = await toolbar.locator("button, select").evaluateAll((elements) => elements.map((element) => ({
      disabled: element instanceof HTMLButtonElement || element instanceof HTMLSelectElement ? element.disabled : false,
      label: element.getAttribute("aria-label") ?? element.textContent?.trim() ?? "",
      pressed: element.getAttribute("aria-pressed")
    })));
    assert.deepEqual(controls.map((control) => control.label), [
      "Undo", "Redo", "Heading", "Bold", "Italic", "Strikethrough", "Bulleted list", "Numbered list", "Blockquote", "Link"
    ], `unexpected toolbar order: ${JSON.stringify(controls)}`);
    assert.ok(controls.slice(3, 9).every((control) => control.pressed === "true" || control.pressed === "false"), "toggle controls need aria-pressed state");
    const editor = await activeEditor(page);
    await editor.click();
    await page.keyboard.press("Control+End");
    await page.keyboard.insertText("a");
    await page.waitForTimeout(600);
    await page.keyboard.insertText("b");
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(50);
    assert.equal(await page.getByRole("button", { name: "Undo", exact: true }).isDisabled(), false, "Undo must be keyboard reachable when history has an earlier transaction");
    assert.equal(await page.getByRole("button", { name: "Redo", exact: true }).isDisabled(), false, "Redo must be keyboard reachable after undo");
    await page.getByRole("button", { name: "Undo", exact: true }).focus();
    await page.keyboard.press("Tab");
    assert.equal(await page.locator(":focus").getAttribute("aria-label"), "Redo");
    await page.keyboard.press("Tab");
    assert.equal(await page.locator(":focus").getAttribute("aria-label"), "Heading");
    await page.getByRole("button", { name: "Link", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Insert link", exact: true });
    const linkInput = dialog.getByRole("textbox", { name: "Link address", exact: true });
    await dialog.waitFor();
    await page.waitForFunction(() => document.activeElement?.matches('input[type="url"]'));
    assert.equal(await linkInput.evaluate((element) => document.activeElement === element), true, "link input must receive focus on open");
    await page.keyboard.press("Shift+Tab");
    assert.equal(await page.locator(":focus").getByText("Cancel", { exact: true }).count(), 1, "Shift+Tab must wrap inside the dialog");
    await page.keyboard.press("Tab");
    assert.equal(await linkInput.evaluate((element) => document.activeElement === element), true, "Tab must wrap back to the link input");
    await page.keyboard.insertText("javascript:alert(1)");
    await page.keyboard.press("Enter");
    await dialog.getByRole("alert").waitFor();
    await page.keyboard.press("Escape");
    await assert.doesNotReject(() => dialog.waitFor({ state: "hidden" }));
    await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "Link");
    assert.equal(await page.locator(":focus").getAttribute("aria-label"), "Link");
    return controls;
  });

  await check("dark, high-contrast, reduced-motion, and read-only states remain usable", async () => {
    await page.getByRole("button", { name: "Toggle dark theme", exact: true }).click();
    assert.equal(await page.locator("main.editor-evaluation").getAttribute("data-theme"), "dark");
    await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
    const screenshot = await page.screenshot();
    assert.ok(screenshot.byteLength > 0, "dark/high-contrast/reduced-motion state did not render");
    await page.emulateMedia({ forcedColors: "none", reducedMotion: "no-preference" });
    await page.getByRole("button", { name: "Enable read only", exact: true }).click();
    const root = page.locator('[data-editor-root="true"]');
    assert.equal(await root.locator('[contenteditable="true"]').count(), 0, "read-only editor remains contenteditable");
    assert.ok(await root.locator('[role="toolbar"] button:disabled').count() >= 9, "read-only toolbar controls must be disabled");
    return "dark/high-contrast/reduced-motion/read-only states rendered";
  });

  await check("offline load has no runtime external request, failed response, console error, or page error", async () => {
    assert.deepEqual(externalRequests, [], `unexpected runtime network requests: ${externalRequests.join(", ")}`);
    assert.deepEqual(failedResponses, [], `local evaluation responses failed: ${failedResponses.join(" | ")}`);
    assert.deepEqual(consoleErrors, [], `console errors: ${consoleErrors.join(" | ")}`);
    assert.deepEqual(pageErrors, [], `page errors: ${pageErrors.join(" | ")}`);
    return "offline browser observation clean";
  });
} catch (error) {
  const details = String(error?.stack ?? error);
  matrixAbort ??= { id: activeCheckId ?? "matrix setup", details };
  if (!checks.some((check) => check.id === matrixAbort.id && check.details === details)) {
    checks.push({ id: matrixAbort.id, status: "fail", details, elapsedMs: 0 });
  }
  console.error(`MATRIX_ABORT=${matrixAbort.id}: ${details}`);
} finally {
  await browser?.close();
  if (ownsServer) {
    await stopOwnedVite();
    await assertPortFree();
  }
}

const report = {
  candidate: "@tiptap/react@3.29.2 + @tiptap/starter-kit@3.29.2 + @tiptap/markdown@3.29.2",
  baseUrl,
  checks,
  consoleErrors,
  pageErrors,
  externalRequests,
  failedResponses,
  matrixAbort
};
console.log(JSON.stringify(report, null, 2));
console.table(checks.map(({ id, status }) => ({ status, id })));
const failures = checks.filter((check) => check.status === "fail");
if (failures.length) {
  console.error(`${failures.length} clipping note editor browser matrix checks failed.`);
  process.exitCode = 2;
} else {
  console.log("Clipping note editor browser matrix passed.");
}
