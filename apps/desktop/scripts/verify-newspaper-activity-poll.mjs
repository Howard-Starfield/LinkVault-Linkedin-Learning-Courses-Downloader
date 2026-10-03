import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";
import ts from "typescript";

// Execute the actual screen effect with controlled IPC, native subscriptions,
// visibility/focus, and timers. This is a lifecycle fixture, not native UAT.
const source = (await readFile(new URL("../src/components/newspaper/NewspaperView.tsx", import.meta.url), "utf8")).replace(/\r\n/g, "\n");
const start = source.indexOf('  useEffect(() => {\n    if (mode === "library" || !isTauriRuntime()) return;');
assert.ok(start >= 0, "The download activity effect must be present");
const end = source.indexOf("  }, [mode]);", start);
assert.ok(end > start);
const effect = ts.transpileModule(source.slice(start, end + "  }, [mode]);".length), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None }
}).outputText;
const events = ["newspaper://activity-invalidated", "newspaper://optimization-progress", "newspaper://library-invalidated"];
const flush = async () => { for (let i = 0; i < 24; i++) await Promise.resolve(); };
const snapshot = (live) => ({ jobs: [], batches: [], progress: [], schedules: [], optimizationRuntime: {}, hasLiveActivity: live });

function harness({ deferredListeners = false, failedListener = null, deferredBootstrap = false, mode = "downloads", native = true } = {}) {
  const timers = new Map();
  const requests = [];
  const subscriptions = [];
  const nativeListeners = new Map();
  const domListeners = new Map();
  const commands = [];
  let timerId = 0;
  let cleanup;
  let appliedSnapshots = 0;
  let appliedState = 0;
  let bootstrapRequest;
  let visibility = "visible";
  let subscriptionFailure = failedListener;
  let unlistened = 0;
  runInNewContext(effect, {
    mode, useEffect(callback) { cleanup = callback(); }, isTauriRuntime: () => native,
    window: {
      setTimeout(callback, delay) { const id = ++timerId; timers.set(id, { callback, delay }); return id; },
      clearTimeout(id) { timers.delete(id); },
      addEventListener(event, callback) { domListeners.set(`window:${event}`, callback); },
      removeEventListener(event, callback) { assert.equal(domListeners.get(`window:${event}`), callback); domListeners.delete(`window:${event}`); }
    },
    document: {
      get visibilityState() { return visibility; },
      addEventListener(event, callback) { domListeners.set(`document:${event}`, callback); },
      removeEventListener(event, callback) { assert.equal(domListeners.get(`document:${event}`), callback); domListeners.delete(`document:${event}`); }
    },
    invoke(command) {
      commands.push(command);
      assert.equal(nativeListeners.size, events.length, "Every native invalidation subscription must precede state reads");
      if (command === "refresh_newspaper_catalog") return Promise.resolve([]);
      if (command === "bootstrap_newspaper_state") {
        const bootstrap = { catalog: [], jobs: [], batches: [], schedules: [] };
        if (!deferredBootstrap) return Promise.resolve(bootstrap);
        return new Promise((resolve) => { bootstrapRequest = () => resolve(bootstrap); });
      }
      assert.equal(command, "get_newspaper_activity_snapshot");
      assert.equal(requests.length, 0, "There must never be concurrent activity IPC requests");
      return new Promise((resolve, reject) => requests.push({ resolve, reject }));
    },
    listen(event, callback) {
      assert.ok(events.includes(event));
      nativeListeners.set(event, callback);
      const unlisten = () => { nativeListeners.delete(event); unlistened++; };
      if (subscriptionFailure === event) {
        subscriptionFailure = null;
        nativeListeners.delete(event);
        return Promise.reject(new Error("Synthetic subscription failure"));
      }
      if (deferredListeners) return new Promise((resolve) => subscriptions.push(() => resolve(unlisten)));
      return Promise.resolve(unlisten);
    },
    FALLBACK_CATALOG: [],
    toast: { error() {} },
    setCatalog() { appliedState++; },
    setJobs(value) { appliedState++; if (typeof value === "function") appliedSnapshots++; },
    setBatches() { appliedState++; }, setJobProgress() { appliedState++; },
    setSchedules() { appliedState++; }, setOptimizationRuntime() { appliedState++; },
    sameSnapshotList: () => true, sameSnapshotRecord: () => true
  });
  return {
    timers, requests, subscriptions, commands, nativeListeners, domListeners,
    get appliedSnapshots() { return appliedSnapshots; }, get appliedState() { return appliedState; }, get unlistened() { return unlistened; },
    cleanup() { cleanup?.(); },
    onlyTimer(delay) { assert.equal(timers.size, 1, "Exactly one timer must own the next refresh"); assert.equal([...timers.values()][0].delay, delay); },
    fireTimer() { assert.equal(timers.size, 1); const [id, timer] = [...timers.entries()][0]; timers.delete(id); timer.callback(); },
    emit(event = events[0]) { nativeListeners.get(event)?.(); },
    focus() { domListeners.get("window:focus")?.(); },
    visibility(value) { visibility = value; domListeners.get("document:visibilitychange")?.(); },
    resolveBootstrap() { bootstrapRequest(); },
    async start() { this.onlyTimer(0); this.fireTimer(); await flush(); },
    async resolve(live = false) { assert.equal(requests.length, 1); requests.shift().resolve(snapshot(live)); await flush(); },
    async reject() { assert.equal(requests.length, 1); requests.shift().reject(new Error("Synthetic IPC failure")); await flush(); }
  };
}

// Idle entry does one snapshot, then no timer and no completed-history reads.
const idle = harness();
await idle.start();
await idle.resolve(false);
assert.equal(idle.timers.size, 0);
const idleCommands = idle.commands.length;
await flush();
assert.equal(idle.commands.length, idleCommands);
assert.equal(idle.commands.filter((command) => command === "bootstrap_newspaper_state").length, 1);

// Queue, optimization, library, and focus invalidations wake a quiet screen.
for (const event of events) {
  idle.emit(event); idle.emit(event);
  idle.onlyTimer(100); idle.fireTimer(); await flush(); await idle.resolve(false);
  assert.equal(idle.timers.size, 0);
}
idle.focus(); idle.onlyTimer(100); idle.fireTimer(); await flush(); await idle.resolve(false);
assert.equal(idle.commands.filter((command) => command === "bootstrap_newspaper_state").length, 1, "Wakeups must not reload completed history through bootstrap");
idle.cleanup();
assert.equal(idle.unlistened, events.length);
assert.equal(idle.domListeners.size, 0);

// Visible live work retains 1-second progress; hidden work has no timer.
const active = harness();
await active.start(); await active.resolve(true); active.onlyTimer(1000);
active.fireTimer(); await flush(); await active.resolve(true); active.onlyTimer(1000);
active.visibility("hidden"); assert.equal(active.timers.size, 0);
active.emit(); active.focus(); assert.equal(active.timers.size, 0);
active.visibility("visible"); active.onlyTimer(100); active.fireTimer(); await flush();
await active.resolve(true); active.onlyTimer(1000);
active.fireTimer(); await flush(); await active.resolve(false); assert.equal(active.timers.size, 0);
active.cleanup();

// In-flight event bursts coalesce into exactly one follow-up refresh.
const burst = harness();
await burst.start();
for (let i = 0; i < 20; i++) burst.emit(events[i % events.length]);
burst.focus(); assert.equal(burst.timers.size, 0); assert.equal(burst.requests.length, 1);
await burst.resolve(false); burst.onlyTimer(100); burst.fireTimer(); await flush();
await burst.resolve(false); assert.equal(burst.timers.size, 0); burst.cleanup();

// A startup event between listener acknowledgement and the initial snapshot is retained.
const startup = harness({ deferredListeners: true });
await startup.start(); assert.equal(startup.commands.length, 0);
startup.subscriptions.shift()(); await flush(); startup.emit();
assert.equal(startup.commands.length, 0, "State reads must wait for all listeners");
startup.subscriptions.splice(0).forEach((resolve) => resolve()); await flush();
assert.equal(startup.requests.length, 1);
await startup.resolve(false); startup.onlyTimer(100); startup.fireTimer(); await flush();
await startup.resolve(false); assert.equal(startup.timers.size, 0); startup.cleanup();

// Failures retry at 15 seconds; event bursts neither accelerate nor starve retries.
const retry = harness();
await retry.start(); await retry.reject(); retry.onlyTimer(15000);
const retryTimer = [...retry.timers.keys()][0];
retry.emit(); retry.focus(); retry.emit(events[1]); retry.onlyTimer(15000);
assert.equal([...retry.timers.keys()][0], retryTimer, "Event bursts must preserve the retry deadline");
retry.fireTimer(); await flush(); await retry.reject(); retry.onlyTimer(15000);
retry.fireTimer(); await flush(); await retry.resolve(false); assert.equal(retry.timers.size, 0); retry.cleanup();

// Partial listener failures release successful subscriptions and retry setup.
const subscriptionRetry = harness({ failedListener: events[1] });
await subscriptionRetry.start(); assert.equal(subscriptionRetry.commands.length, 0);
assert.equal(subscriptionRetry.nativeListeners.size, 0); subscriptionRetry.onlyTimer(15000);
subscriptionRetry.fireTimer(); await flush(); await subscriptionRetry.resolve(false);
assert.equal(subscriptionRetry.timers.size, 0); subscriptionRetry.cleanup();

// Disposal prevents late snapshots and late initialization from touching state.
const stale = harness(); await stale.start();
const staleCallback = stale.nativeListeners.get(events[0]);
const appliedBeforeCleanup = stale.appliedState;
stale.cleanup(); staleCallback(); await stale.resolve(true);
assert.equal(stale.appliedState, appliedBeforeCleanup); assert.equal(stale.timers.size, 0);
assert.equal(stale.nativeListeners.size, 0); assert.equal(stale.domListeners.size, 0);
const pendingSubscriptions = harness({ deferredListeners: true }); await pendingSubscriptions.start();
pendingSubscriptions.subscriptions.shift()(); await flush();
pendingSubscriptions.cleanup();
assert.equal(pendingSubscriptions.unlistened, 1, "Already acknowledged subscriptions must dispose before the remaining acknowledgements arrive");
pendingSubscriptions.subscriptions.splice(0).forEach((resolve) => resolve()); await flush();
assert.equal(pendingSubscriptions.nativeListeners.size, 0); assert.equal(pendingSubscriptions.unlistened, events.length);
assert.equal(pendingSubscriptions.commands.length, 0); assert.equal(pendingSubscriptions.timers.size, 0);
const staleBootstrap = harness({ deferredBootstrap: true }); await staleBootstrap.start();
staleBootstrap.cleanup(); staleBootstrap.resolveBootstrap(); await flush();
assert.equal(staleBootstrap.appliedState, 0); assert.equal(staleBootstrap.requests.length, 0); assert.equal(staleBootstrap.timers.size, 0);

const disposedTimer = harness(); await disposedTimer.start(); await disposedTimer.resolve(true);
const staleTimerCallback = [...disposedTimer.timers.values()][0].callback;
const commandsBeforeTimerDisposal = disposedTimer.commands.length;
disposedTimer.cleanup(); staleTimerCallback(); await flush();
assert.equal(disposedTimer.commands.length, commandsBeforeTimerDisposal); assert.equal(disposedTimer.timers.size, 0);

for (const options of [{ mode: "library" }, { native: false }]) {
  const skipped = harness(options); assert.equal(skipped.timers.size, 0); assert.equal(skipped.commands.length, 0); assert.equal(skipped.domListeners.size, 0);
}
console.log("Newspaper activity effect passed: idle quiet, visible live cadence, event/focus/visibility wakes, listener-first startup, single-flight bursts, bounded retries, and complete disposal.");
