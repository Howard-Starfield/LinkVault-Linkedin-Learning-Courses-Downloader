import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";

// Exercise the actual App owner with controlled IPC settlement, without a
// native queue or modifying user data. Remove only its TypeScript signature.
const source = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
const start = source.indexOf("  function ensureNewspaperQueueProcessing(");
assert.ok(start >= 0, "The existing newspaper queue owner must remain present");
const end = source.indexOf("\n  }", start) + "\n  }".length;
assert.ok(end > start, "The owner must have a bounded extraction point");
assert.ok(!source.includes("processNewspaperSchedules"), "Native scheduling must replace the startup/15-second renderer processing effect");
assert.ok(source.includes("ensureNewspaperQueueProcessing(options ?? null, true)"), "Explicit manual queue processing must remain serialized by the App owner");
const owner = source.slice(start, end).replace(
  /function ensureNewspaperQueueProcessing\([\s\S]*?\) \{/,
  "function ensureNewspaperQueueProcessing(options = null, rearm = false) {"
);
const ref = { current: null };
const calls = [];
const pending = [];
const process = runInNewContext(`${owner}\nensureNewspaperQueueProcessing`, {
  newspaperQueuePromiseRef: ref,
  invoke(command) {
    calls.push(command);
    return new Promise((resolve, reject) => pending.push({ resolve, reject }));
  }
});
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
async function settlePass(failure = false) {
  await flush();
  assert.equal(pending.length, 1);
  if (failure) pending.shift().reject(new Error("Synthetic IPC failure"));
  else {
    pending.shift().resolve();
    await flush();
    assert.equal(pending.length, 1);
    pending.shift().resolve();
  }
  await flush();
}

const first = process();
assert.equal(process(), first, "Duplicate requests must reuse an in-flight pass");
await settlePass();
await first;
assert.equal(ref.current, null, "A settled pass must release the owner so a later request can process new work");

const second = process();
await flush();
const third = process(null, true);
assert.notEqual(third, second, "Explicit rearm must enqueue a subsequent pass");
assert.equal(process(), third, "Duplicate requests must reuse the most recently scheduled pass");
await settlePass();
await second;
assert.equal(ref.current, third, "An older settlement must not clear a newer pending pass");
await settlePass();
await third;
assert.equal(ref.current, null, "The final serialized pass must release the owner");

const failed = process();
const rejection = assert.rejects(failed, /Synthetic IPC failure/);
await settlePass(true);
await rejection;
assert.equal(ref.current, null, "A failed pass must release the owner for retry");
const retry = process();
await settlePass();
await retry;
assert.deepEqual(calls, [
  "process_newspaper_queue", "process_newspaper_optimization_queue",
  "process_newspaper_queue", "process_newspaper_optimization_queue",
  "process_newspaper_queue", "process_newspaper_optimization_queue",
  "process_newspaper_queue",
  "process_newspaper_queue", "process_newspaper_optimization_queue"
]);
console.log("Newspaper queue owner passed: settled/rejected promises release, rearmed passes serialize, and duplicate requests reuse in-flight work.");
