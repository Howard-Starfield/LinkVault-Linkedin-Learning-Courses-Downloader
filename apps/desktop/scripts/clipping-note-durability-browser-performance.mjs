import assert from "node:assert/strict";

const SAMPLE_COUNT = 20;

function summarize(scenario, samples) {
  const sorted = [...samples].sort((left, right) => left - right);
  const rank = (percentile) => sorted[Math.ceil(sorted.length * percentile / 100) - 1];
  return {
    scenario,
    samples: sorted.length,
    p50Ms: Number(rank(50).toFixed(3)),
    p95Ms: Number(rank(95).toFixed(3)),
    maxMs: Number(sorted.at(-1).toFixed(3))
  };
}

async function prepare(page, token, reason = "close") {
  return page.evaluate(async ({ requestToken, requestReason }) => {
    const started = performance.now();
    await window.__CLIPPING_NOTE_EXIT_BRIDGE__.emitPrepare({
      token: requestToken,
      reason: requestReason,
      deadlineMs: 15_000
    });
    const resolution = window.__CLIPPING_NOTE_EXIT_BRIDGE__.resolutions.find(
      (entry) => entry.token === requestToken
    );
    return { elapsedMs: performance.now() - started, resolution };
  }, { requestToken: token, requestReason: reason });
}

async function measureExitScenario(page, title, scenario, startToken, setup) {
  const samples = [];
  for (let index = 0; index < SAMPLE_COUNT; index += 1) {
    await setup(index);
    const measured = await prepare(page, startToken + index, scenario === "checkpoint-fallback" ? "exit" : "close");
    assert.deepEqual(measured.resolution, { token: startToken + index, durable: true });
    samples.push(measured.elapsedMs);
    await page.getByText(scenario === "checkpoint-fallback" ? "Recovered draft saved locally." : "Saved", { exact: true }).waitFor();
  }
  return summarize(scenario, samples);
}

async function measureEditorHeap(page) {
  const session = await page.context().newCDPSession(page);
  const rows = [];
  for (const clippingCount of [500, 50, 8]) {
    await page.evaluate((count) => {
      window.__CLIPPING_LIBRARY_TEST__.details = window.__CLIPPING_LIBRARY_TEST__.details.slice(0, count);
    }, clippingCount);
    await session.send("HeapProfiler.collectGarbage");
    const heap = await session.send("Runtime.getHeapUsage");
    rows.push({ clippingCount, usedBytes: Math.round(heap.usedSize), totalBytes: Math.round(heap.totalSize) });
  }
  await session.detach();
  return rows;
}

export async function measureClippingNoteDurabilityBrowser(page, title) {
  let token = 10_000;
  const exitLatency = [];
  exitLatency.push(await measureExitScenario(page, title, "clean", token, async () => {}));
  token += SAMPLE_COUNT;
  exitLatency.push(await measureExitScenario(page, title, "dirty", token, async (index) => {
    await title.fill(`Exit dirty latency ${index}`);
  }));
  token += SAMPLE_COUNT;
  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.updateDelayMs = 120; });
  exitLatency.push(await measureExitScenario(page, title, "in-flight", token, async (index) => {
    await title.fill(`Exit in-flight latency ${index}`);
    await page.getByText("Saving…", { exact: true }).waitFor();
  }));
  token += SAMPLE_COUNT;
  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.updateDelayMs = 30; });
  exitLatency.push(await measureExitScenario(page, title, "checkpoint-fallback", token, async (index) => {
    await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.failNext = true; });
    await title.fill(`Exit checkpoint fallback ${index}`);
    await page.waitForTimeout(525);
  }));

  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.checkpointFail = true; });
  await title.fill("Lifecycle blocked draft");
  const blocked = await prepare(page, token + SAMPLE_COUNT);
  assert.deepEqual(blocked.resolution, { token: token + SAMPLE_COUNT, durable: false });
  assert.equal(await title.inputValue(), "Lifecycle blocked draft", "blocked close discarded the visible draft");
  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.checkpointFail = false; });
  await page.getByRole("button", { name: "Retry" }).click();
  await page.getByText("Saved", { exact: true }).waitFor();

  return { exitLatency, editorHeap: await measureEditorHeap(page) };
}
