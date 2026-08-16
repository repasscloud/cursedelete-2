# ADR-0003: Adaptive worker model

## Status

Accepted.

## Context

Optimal concurrency for a deletion workload depends entirely on the
target: local NVMe keeps benefiting from more workers well past 128;
high-latency SMB peaks in the dozens and then *degrades* from contention
and server-side throttling; a single spinning HDD can be actively hurt by
any concurrency at all due to seek thrashing. Neither prior implementation
attempted this -- `_old/CurseDelete` uses a fixed
`Math.Clamp(Environment.ProcessorCount * 2, 4, 64)`, `_old/sfvdd` exposes a
manual `--threads` flag only. `CPU count * N` is explicitly called out in
the product brief as insufficient because the bottleneck for network and
remote targets is not CPU at all.

## Decision

Two independent mechanisms:

1. **Pre-spawned, semaphore-gated worker threads**
   (`cursdel_core::sync::ResizableSemaphore`,
   `pipeline::file_worker_loop`). Up to the configured ceiling (256 for
   `--workers auto`, or exactly `n` for `--workers n`) of ordinary OS
   threads are spawned once and block on a resizable counting semaphore
   before pulling work. Growing/shrinking concurrency is then just
   changing a number (`set_capacity`), never spawning or joining OS
   threads at runtime. This directly satisfies "do not create one OS
   thread per file" and "bounded dynamically managed worker pool" without
   the complexity of actually starting/stopping threads under load. A
   blocked idle thread costs a kernel stack and nothing else, so
   pre-spawning the ceiling is cheap even when the controller settles far
   below it.

2. **A pure hill-climbing decision function**
   (`cursdel_core::adaptive::AdaptiveController::on_sample`). Every 400ms
   the coordinator drains a window of completion/error counters
   (`WindowSampler`) and feeds them in; the controller returns
   `Increase(n)` / `Decrease(n)` / `Hold`. The algorithm:
   - backs off immediately if the window's error rate exceeds 15%,
     regardless of throughput trend (protects an overloaded/throttling
     target first, before trying to be clever about throughput);
   - otherwise compares this window's throughput to the last, growing
     while growth keeps helping (>=5% improvement), shrinking (using the
     same threshold in the opposite direction) if the last move hurt, and
     holding within a 5% flat band;
   - while holding, re-probes periodically (every 6 held windows),
     **alternating** the probe direction between grow and shrink. An
     earlier version always re-probed by growing, which produced a
     systematic upward drift over long-running operations even after
     finding the true optimum -- a probe that turns out to hurt is only
     partially corrected by shrinking back, so one-directional probing
     never nets to zero. Alternating direction cancels the drift; see the
     `converges_near_peak_of_concave_throughput_curve` regression test,
     which failed under one-directional probing before this fix.

`on_sample` is deliberately pure (`WindowStats -> AdaptiveDecision`, no I/O,
no threads) specifically so it can be validated without real storage
hardware, which this development/CI environment does not have one of each
of (NVMe, HDD, high-latency SMB, ...). Instead, unit tests drive it against
synthetic throughput-as-a-function-of-workers curves representative of
each target class:

- `converges_near_peak_of_concave_throughput_curve`: a downward parabola
  peaking at 24 workers (representative of SMB-like contention) --
  controller must settle within +/-6 of the peak.
- `climbs_to_near_max_when_more_workers_always_helps`: throughput grows
  with diminishing returns and never falls (representative of local NVMe)
  -- controller must climb to near the configured ceiling.
- `shrinks_toward_min_when_concurrency_always_hurts`: throughput is
  strictly `1/workers` (representative of a single spinning HDD) --
  controller must settle near the configured minimum.

This is validation of the *algorithm's convergence behaviour*, not a
benchmark of real hardware throughput -- see `docs/BENCHMARKS.md` for what
still requires real target hardware to validate authoritatively.

`--workers <n>` bypasses the controller entirely: no `AdaptiveController`
is even constructed (see `pipeline::run`, `WorkerPolicy::Fixed`), so manual
worker counts are exactly deterministic, satisfying "manual worker count
must be deterministic."

## Consequences

- The controller can be tuned (threshold constants in `adaptive.rs`)
  without touching the threading model, and vice versa.
- Real-world tuning of the threshold constants against actual NVMe/SSD/
  HDD/SMB hardware remains open work -- see `docs/BENCHMARKS.md` for the
  harness and what running it on suitable hardware would validate.
- Separate concurrency pools for directory removal vs. file deletion (see
  ADR-0002) mean the adaptive controller only ever governs the file-delete
  pool; directory throughput is intentionally not adaptive since it is
  cheap and not expected to be the bottleneck.
