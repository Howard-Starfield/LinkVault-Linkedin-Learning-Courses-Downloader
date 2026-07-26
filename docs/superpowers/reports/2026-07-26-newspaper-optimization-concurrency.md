# Newspaper Optimization Phase 0 Benchmark

**Date:** 2026-07-26  
**Status:** Automated benchmark complete; native desktop responsiveness UAT remains open  
**Roadmap:** [Newspaper Optimization Recovery and Throughput Roadmap](../specs/2026-07-26-newspaper-optimization-recovery-roadmap.md)

## Decision

- Keep `20` as the advanced hard ceiling.
- Use adaptive `Auto` by default.
- On the measured 16-logical-processor, 64-GB-class Windows machine, let Auto ramp from 2 workers toward 16 when resource pressure remains safe.
- Do not default to 20. In the evenly batched steady-state test, 20 workers did not improve throughput over the best 16-worker result, while page latency and memory increased.
- Keep libwebp internal threading enabled through 16 active page workers. At 20 workers, internal threading did not help; Phase 3 may disable it at that ceiling.
- Treat the workload as CPU- and decoded-memory-heavy. File reads and writes were a small fraction of conversion time; decode, encode, and validation decode dominated.

The resource policy remains provisional until native desktop scrolling and input responsiveness are checked while the benchmark is active.

## Benchmark Harness

The existing `newspaper_webp_bench` example was expanded into a production-like concurrent pipeline:

1. read the source image;
2. decode and convert to RGB or RGBA;
3. encode with libwebp method 2;
4. write an atomic `.webp.part` output;
5. decode the encoded bytes and verify dimensions;
6. rename the temporary output;
7. delete all benchmark outputs with the temporary benchmark directory.

It reports JSON with:

- independent counterbalanced trials;
- worker and encoder-thread settings;
- pages per minute;
- median and 95th-percentile page latency;
- read, decode/convert, encode, write, validation, and rename timing;
- process CPU core-equivalents and percent of the machine;
- peak working set and private memory;
- minimum available system memory;
- source/output byte ratio and observed dimensions.

The benchmark never changes the newspaper library or its database.

## Dataset and Method

- Two real downloaded JPEG pages were used:
  - 2150 by 2400 pixels;
  - 2500 by 4384 pixels.
- Quality 45 was used for the concurrency comparison.
- The initial matrix covered 1, 2, 4, 8, 12, 16, and 20 workers with libwebp threading both off and on.
- Three counterbalanced trials removed run-order bias.
- A second steady-state comparison used 240 conversions per scenario. The task count is divisible by 8, 12, 16, and 20, avoiding partial final-batch bias.
- A separate three-trial sweep covered every supported quality preset.
- Output dimensions were validated after every conversion.

Raw local JSON artifacts are under `apps/desktop/src-tauri/target/newspaper-webp-bench/` and are intentionally not source-controlled.

## Steady-State Concurrency Result

Each row converted 240 real pages through the full benchmark pipeline.

| Encoder internal threads | Page workers | Pages/min | Median page | P95 page | CPU | Peak private memory |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 8 | 621.2 | 549 ms | 1,104 ms | 49.3% | 752 MiB |
| 1 | 12 | 720.2 | 815 ms | 1,481 ms | 68.8% | 1,103 MiB |
| 1 | 16 | **808.9** | 910 ms | 1,675 ms | 79.7% | 1,441 MiB |
| 1 | 20 | 791.6 | 1,270 ms | 2,222 ms | 78.7% | 1,726 MiB |
| 0 | 8 | 603.5 | 562 ms | 1,137 ms | 49.0% | 768 MiB |
| 0 | 12 | 722.9 | 820 ms | 1,416 ms | 68.6% | 1,104 MiB |
| 0 | 16 | 760.1 | 1,155 ms | 1,890 ms | 74.9% | 1,370 MiB |
| 0 | 20 | 803.9 | 1,354 ms | 2,185 ms | 79.7% | 1,796 MiB |

Interpretation:

- The best observed result was 16 workers with encoder threading enabled.
- Twenty workers remained safe on this machine, but did not materially beat the best 16-worker throughput.
- Compared with 16 threaded workers, 20 threaded workers used about 20% more private memory, raised median page latency about 40%, and reduced throughput about 2%.
- Disabling internal threads at 20 recovered a small amount of throughput, but still did not justify making 20 the default.

The three-trial single-worker median was 97.1 pages/minute at the current encoder-thread setting. The best steady-state result is about 8.3 times that rate.

## Where Time Goes

A separate 240-page run at 16 workers split the production-like stages:

| Stage | Median time/page |
| --- | ---: |
| Read source | 1 ms |
| Decode and RGB conversion | 211 ms |
| WebP encode | 617 ms |
| Write temporary WebP | 23 ms |
| Validation decode | 452 ms |
| Atomic rename | 13 ms |

Percentiles from different stages do not add exactly to the median total because they can come from different pages. The direction is nevertheless clear: image processing dominates. Disk read/write and rename are small compared with decode, encode, and validation. CPU monitoring is useful, while the memory governor must account for several simultaneously decoded image buffers.

## Quality Sweep

Median of three trials, one page worker, two pages repeated twice per trial:

| Quality | Pages/min | Median page | Encoded/source ratio |
| ---: | ---: | ---: | ---: |
| 25 | 116.6 | 325 ms | 0.341 |
| 35 | 116.2 | 322 ms | 0.392 |
| 45 | 112.1 | 351 ms | 0.449 |
| 55 | 105.7 | 354 ms | 0.495 |
| 74 | 100.3 | 396 ms | 0.584 |
| 86 | 96.5 | 399 ms | 0.814 |
| 92 | 89.0 | 432 ms | 1.025 |

Quality 92 was slightly larger than these two JPEG inputs. Production already keeps the original when the encoded file is not smaller, so this does not create a storage regression.

## Provisional Auto Policy

```text
auto CPU ceiling = min(20, logical processors)
memory ceiling =
  floor((available memory - required reserve) / conservative worker budget)
admitted workers =
  min(user ceiling, auto CPU ceiling, memory ceiling)
```

Initial constants for Phase 3:

- start with 2 workers;
- add no more than 1 worker per adjustment window;
- use 160 MiB as the conservative initial per-worker budget;
- preserve at least 4 GiB or 10% of installed memory, whichever is greater;
- target approximately 80% total CPU;
- reduce workers after sustained CPU above 90%, low-memory pressure, allocation failure, or responsiveness degradation;
- Auto may reach 16 on the reference machine;
- manual mode may request up to 20, subject to the emergency memory guard.

The measured incremental private-memory cost was generally below 100 MiB per additional worker, but 160 MiB leaves room for an all-large-page workload and allocator variation.

## Reproduction

From `apps/desktop/src-tauri`:

```powershell
cargo build --release --example newspaper_webp_bench

.\target\release\examples\newspaper_webp_bench.exe `
  --input <small-page.jpg> `
  --input <large-page.jpg> `
  --workers 1,2,4,8,12,16,20 `
  --qualities 45 `
  --encoder-threads 0,1 `
  --repetitions 10 `
  --trials 3 `
  --output target/newspaper-webp-bench/concurrency.json
```

For an evenly batched high-concurrency comparison, use 120 repetitions with the two inputs, producing 240 tasks per scenario.

## Remaining Phase 0 Gate

- Run native desktop UAT while 16 and 20 workers are active.
- Confirm newspaper scrolling, zoom, navigation, pause, and cancellation remain responsive.
- If 16 workers causes visible input latency, lower the initial Auto CPU ceiling while retaining 16 and 20 as manual ceilings.

Database commits, lease recovery, and crash injection are intentionally deferred to Phase 1.
