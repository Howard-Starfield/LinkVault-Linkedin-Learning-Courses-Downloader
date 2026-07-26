# Newspaper Optimization Recovery and Throughput Roadmap

**Status:** Phase 0 automated benchmark complete; native responsiveness UAT pending; Phases 1–4 not started  
**Scope:** LinkVault desktop newspaper downloads and post-download image optimization  
**Primary owners:** `apps/desktop/src-tauri/src/newspaper/optimization_service.rs`, `optimizer.rs`, `library_events.rs`, and `apps/desktop/src/components/NewspaperView.tsx`

## Outcome

Make post-download image optimization fast, observable, and crash-safe:

- A computer or app crash never loses a successfully downloaded newspaper page.
- Restarting resumes unfinished work without restarting the entire edition.
- Optimization can use a bounded swarm of up to 20 page workers.
- The default worker count adapts to CPU and available memory instead of assuming 20 workers are safe.
- The download surface reports exact stage progress rather than a generic `Processing` state.

## Current-State Findings

The current optimizer is restartable in a limited, implicit way:

- It queries completed pages whose `optimized_path` is still empty.
- Each output is written to a `.webp.part` file and renamed after validation.
- The page record is updated after each successful conversion.
- Original images are retained until the edition finishes optimizing.

This is a good safety foundation, but it is not yet a durable recovery system. A crash can leave an expired in-flight task, a stale `.part` file, a valid WebP that was renamed before its database update, or an original file that was not cleaned up after the database commit. The next startup can usually reprocess these pages, but it cannot explain or reconcile every state deliberately.

Optimization is currently sequential across editions and pages. Each page is fully decoded, converted to RGB or RGBA, encoded by libwebp, and decoded again for output validation. The largest currently observed page is 2500 by 4384 pixels. One RGB pixel buffer for that page is about 31 MiB before accounting for the source bytes, decoded image, converted buffer, encoded output, and validation decode. A 20-worker default could therefore consume several gigabytes and oversubscribe the CPU because libwebp also has internal threading enabled.

The current UI percentage reflects downloaded and failed pages only. During optimization it shows `Optimizing images`, and the global activity message says `Downloading and validating pages…`. It does not expose optimized page counts, active workers, throughput, failures, or recovery activity.

## Product Contract

### Progress semantics

Progress must be stage-specific:

1. **Downloading** — received pages out of expected pages.
2. **Validating** — pages accepted or rejected.
3. **Optimizing** — pages converted or deliberately kept in original format.
4. **Finalizing** — database and optional source cleanup.
5. **Complete** — all required stages are terminal.

No combined percentage may show 100% while a required stage still has pending work. If a total is not known, show counts and an indeterminate bar instead of inventing a percentage.

Example row:

> Downloaded 92/92 · Optimized 47/92 · 6 workers · 3.2 pages/s · about 14s

### Recovery semantics

- A downloaded original remains the safety copy until its optimized output is validated and durably committed.
- A page is never assigned to two live workers.
- Work is checkpointed per page, not only per edition.
- Restart reconciliation adopts an already valid optimized output instead of re-encoding it.
- Retriable failures return to the queue with a capped attempt count.
- Permanent failures remain visible and preserve the original page.
- Cleanup may leave redundant files after a crash, but it must never remove both usable copies.

### Concurrency semantics

- `20` is the hard ceiling, not the shipped default.
- **Auto** is the default mode.
- Manual choices may expose 2, 4, 8, 12, 16, and 20 workers under an advanced control.
- Auto begins conservatively, samples resource pressure, and ramps up or down.
- The user interface remains responsive while optimization runs.

## Proposed Durable Data Model

Add a page-level task ledger rather than deriving all state from `newspaper_pages.optimized_path`.

### `newspaper_optimization_tasks`

| Column | Purpose |
| --- | --- |
| `page_id` | Primary key and foreign key to the newspaper page |
| `job_id` | Edition/job grouping and query index |
| `status` | `pending`, `running`, `succeeded`, `kept_original`, or `failed` |
| `attempts` | Number of claimed attempts |
| `lease_owner` | Unique optimizer process/worker owner |
| `lease_expires_at` | Enables recovery from app or computer crashes |
| `started_at` | Current/most recent attempt start |
| `completed_at` | Terminal completion time |
| `source_path` | Source identity used by the task |
| `source_size` | Cheap source-change check |
| `source_modified_at` | Cheap source-change check |
| `output_path` | Intended optimized output |
| `source_bytes` | Progress and savings reporting |
| `output_bytes` | Progress and savings reporting |
| `elapsed_ms` | Throughput and ETA input |
| `last_error` | User-visible diagnostic |
| `error_kind` | Retriable versus permanent classification |

The existing `newspaper_pages.optimized_path` remains the reader-facing committed result. The task ledger owns execution and recovery state.

## Runtime Architecture

### Worker pool

- Use a bounded async coordinator with CPU work dispatched through `spawn_blocking`.
- Use one coordinator to claim tasks and persist results in short SQLite transactions.
- Workers do image I/O and encoding; they do not share a mutable SQLite connection.
- A page lease is acquired transactionally before dispatch.
- The coordinator publishes progress after each terminal page result.
- Pause or cancel is checked between pages; running encodes finish their atomic checkpoint before stopping.

### Adaptive governor

Initial Auto policy:

```text
allowed workers =
  min(
    user ceiling,
    logical CPU count minus one,
    available optimization memory / estimated peak bytes per worker
  )
```

The exact formula must be calibrated in Phase 0. The first conservative memory estimate should be based on page dimensions and multiple decoded buffers, with a minimum allowance per worker. Auto should:

- start with two workers;
- sample CPU and memory about once per second;
- add at most one worker per adjustment window;
- target roughly 80–85% CPU utilization;
- back off on sustained CPU above 90%, low available memory, allocation failure, or UI responsiveness degradation;
- preserve a fixed free-memory reserve and avoid pushing total memory use beyond a safe fraction of installed RAM.

The benchmark must also decide whether libwebp internal threading remains enabled when multiple pages are encoded concurrently. External page concurrency plus internal encoder concurrency can otherwise oversubscribe the CPU.

### Recovery reconciliation

At application startup and before claiming new work:

1. Return expired `running` leases to `pending`.
2. Inspect stale `.part` files and remove only those associated with expired tasks.
3. If the final WebP exists but the database commit is missing, validate its dimensions and source identity and adopt it.
4. If the database is committed but the original remains, schedule safe cleanup rather than re-encoding.
5. Preserve the original and mark a visible failure after the retry limit.

## Phase 0 — Establish the Benchmark and Safety Contract

**Goal:** Measure the real bottleneck and choose safe concurrency policies before changing behavior.

**Dependencies:** None  
**Estimated effort:** One focused implementation/testing session
**Result:** [Phase 0 benchmark report](../reports/2026-07-26-newspaper-optimization-concurrency.md)

### Work

- Create a repeatable optimizer benchmark using representative current pages:
  - a smaller page around 2150 by 2400;
  - a typical newspaper page;
  - the observed maximum around 2500 by 4384.
- Run worker counts 1, 2, 4, 8, 12, 16, and 20.
- Test the supported WebP quality levels used by the product.
- Compare libwebp internal threading on and off under external concurrency.
- Record:
  - pages per minute;
  - median and 95th-percentile page time;
  - CPU utilization;
  - process private memory and working set;
  - peak system memory pressure;
  - output dimensions and readability equivalence;
  - desktop input and scrolling responsiveness.
- Record the current crash states at key file/database boundaries.

### Definition of done

- Benchmark output is checked into a reproducible report or test artifact.
- A safe initial Auto formula and resource reserve are selected from measurements.
- The manual 20-worker mode has documented machine requirements or is disabled when the governor cannot safely admit it.
- The implementation team can distinguish encoder CPU cost from disk and database cost.

## Phase 1 — Durable Page-Level Recovery

**Goal:** Make every page conversion resumable and explainable before adding parallelism.

**Dependencies:** Phase 0 contract  
**Estimated effort:** One to two focused sessions

### Database and backend

- Add and migrate the `newspaper_optimization_tasks` table.
- Backfill tasks for already downloaded pages that still require optimization.
- Add transactional task claim, lease renewal, completion, failure, and retry operations.
- Reconcile expired leases, stale temporary files, valid orphaned outputs, and redundant originals on startup.
- Keep atomic output promotion and validate before committing `optimized_path`.
- Add retriable/permanent error classification and a capped retry policy.
- Check pause/cancel between page checkpoints.
- Add a single-process recovery owner identifier so a second app instance cannot duplicate live work.

### Verification

Add deterministic crash-injection tests at:

- after source decode;
- after `.part` write;
- after validation;
- after rename but before the database commit;
- after the database commit but before original cleanup.

### Definition of done

- Restart resumes only unfinished pages.
- Valid orphaned outputs are adopted without conversion.
- No injected crash loses both the original and optimized page.
- No page is processed by two live workers.
- Retried and permanently failed pages are visible with exact attempt and error state.
- Existing completed libraries remain readable through the migration.

## Phase 2 — Exact Download and Optimization Feedback

**Goal:** Replace generic `Processing` feedback with authoritative stage progress.

**Dependencies:** Phase 1 task counters  
**Estimated effort:** One focused session

### Backend contract

Extend activity snapshots and job models with:

- `download_total`, `download_completed`, `download_failed`;
- `optimization_total`, `optimization_completed`, `optimization_failed`;
- `optimization_pending`, `optimization_recovered`;
- `active_workers`;
- rolling pages-per-minute throughput;
- ETA only after enough stable samples exist;
- original bytes, optimized bytes, and bytes saved;
- current stage and a monotonic progress revision.

Publish a throttled `newspaper://optimization-progress` event after page results, while preserving the existing one-second active polling as a fallback and recovery mechanism.

### Frontend

- Show download and optimization as distinct stages or compact segmented bars.
- Replace `Processing` with the current stage and exact counts.
- Show worker count, throughput, and ETA only while useful.
- Report recovery explicitly, for example `Resuming 18 unfinished pages`.
- Replace the global generic activity message with aggregate download and optimization counts.
- Ensure 100% appears only for a terminal stage and `Complete` only when every required stage is terminal.
- Keep the layout dense enough for the existing desktop download list.

### Definition of done

- A user can answer what is downloading, what is optimizing, what finished, and what failed without opening logs.
- Progress updates appear within about one second.
- Counts remain correct after pausing, restarting, recovering, or running multiple editions.
- A completed download with pending optimization never presents as fully complete.
- Event loss or window suspension self-corrects from the next snapshot.

## Phase 3 — Adaptive Parallel Optimization Swarm

**Goal:** Increase throughput with a bounded pool that protects memory, CPU, and UI responsiveness.

**Dependencies:** Phases 0–2  
**Estimated effort:** Two focused sessions

### Backend

- Replace sequential page processing with the bounded worker architecture.
- Add Auto concurrency with the benchmark-selected initial policy.
- Add CPU and available-memory sampling only while optimization is active.
- Add manual advanced worker ceilings up to 20.
- Keep task claiming, result persistence, and event aggregation in the coordinator.
- Ramp concurrency gradually and reduce it under sustained pressure.
- Ensure one failed page cannot cancel unrelated workers.
- Preserve per-page retry, pause, cancel, and recovery behavior.

### Frontend

- Add a small `Auto` concurrency control with advanced manual choices.
- When active, show compact resource context such as `6 workers · CPU 78% · memory safe`.
- Explain when a requested ceiling is reduced for safety.
- Persist the user's mode and ceiling, but recompute the safe active worker count per run.

### Definition of done

- Throughput improves materially over the Phase 0 single-worker baseline on the reference machine.
- Auto does not exceed the measured CPU and memory safety limits.
- Manual 20-worker mode is a ceiling still governed by emergency memory protection.
- Scrolling, navigation, pause, and cancellation remain responsive.
- Parallel completion order cannot corrupt counts or page ordering.
- App restart during a 20-worker run recovers every unfinished page without duplicate committed work.

## Phase 4 — Stress, Crash, and Release Certification

**Goal:** Prove the combined recovery, progress, and swarm behavior under adverse conditions.

**Dependencies:** Phases 1–3  
**Estimated effort:** One to two focused sessions

### Test matrix

- An edition with at least 500 mixed-size pages.
- Several editions queued together.
- Low available-memory conditions.
- CPU contention from another application.
- Corrupt input image.
- Read-only or full output disk.
- App termination and computer-restart-equivalent process death at every checkpoint.
- Pause, resume, and cancel while workers are active.
- Event interruption followed by polling reconciliation.
- Upgrade from a database without the task ledger.

### Definition of done

- No data loss and no unreadable committed page in the full crash matrix.
- Recovery progress is monotonic and exact.
- Temporary and redundant files are reconciled safely.
- Resource governor behavior matches the Phase 0/3 benchmark contract.
- The final chosen Auto policy, throughput gain, peak memory, and known limitations are documented.
- Native desktop UAT confirms the download list remains responsive and understandable.

## Build Order

| Order | Deliverable | Why it comes here |
| --- | --- | --- |
| 1 | Benchmark harness and policy contract | Prevents guessing about 20-worker performance |
| 2 | Page task ledger and recovery reconciler | Makes later parallel work safe |
| 3 | Authoritative counters and progress UI | Gives visibility before throughput complexity |
| 4 | Adaptive bounded worker pool | Builds on recoverable tasks and observable state |
| 5 | Adverse-condition certification | Validates the complete system rather than isolated parts |

## Deliberately Not Building

- An unbounded or default 20-worker mode.
- A permanent full-system monitoring dashboard; resource details appear only when optimization is active.
- GPU encoding until the CPU benchmark demonstrates that encoder time warrants a separate GPU feasibility study.
- A fabricated overall percentage or early ETA.
- Concurrent deletion of original files before the optimized result is durably committed.
- Broad reader rendering changes; this roadmap covers download and optimization execution and feedback.

## Decisions to Resolve with Phase 0 Evidence

| Decision | Candidate choices |
| --- | --- |
| Encoder threading | libwebp internal threads, external page workers, or a measured combination |
| Memory estimate | Fixed per-worker reserve versus page-dimension-based estimate |
| CPU sampling | Cross-platform system sampler versus Windows-specific implementation |
| Auto ramp interval | Fast ramp for short jobs versus slower, steadier adjustment |
| Manual ceiling behavior | Strict user count versus safe admitted count with explanation |
| ETA window | Page-count rolling average versus size/dimension-weighted estimate |

## Approval Boundary

This document is a plan only. Implementation should begin with Phase 0 and stop at each phase boundary for evidence review. The swarm should not be introduced before the recovery ledger and exact progress counters are working.
