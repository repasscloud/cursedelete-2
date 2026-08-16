# ADR-0002: Streaming enumeration/deletion pipeline

## Status

Accepted.

## Context

The product's non-negotiable requirement: "deletion must begin while
enumeration is still occurring," with memory use bounded regardless of
tree size, and directories removed only after all relevant child
processing is complete.

`_old/CurseDelete` (C#) already demonstrates the right shape: a bounded
`BlockingCollection<string>` queue, with a fixed pool of consumer tasks
started *before* the producer begins walking the tree, so files start
being deleted while the walk is still in progress. Its weaknesses: no
adaptive concurrency (a fixed `DegreeOfParallelism`), a directory list
(`List<string>`) that grows for the lifetime of the whole operation before
being processed in a final reverse-sorted pass (unbounded relative to
directory count, and directories are deleted *after* the entire walk
completes rather than as subtrees finish), and no separation between
"normal delete" and "remediation" work.

## Decision

Four concurrently-running components, connected by bounded
`crossbeam-channel`s that provide backpressure end-to-end:

```text
enumerator thread            (cursdel_core::walk::stream_tree, via a
                               platform engine's DirLister)
      |  EnumEvent (bounded channel)
      v
coordinator (main thread)    (crate::pipeline::coordinator_loop)
      |  registers directories/leaves with DirectoryTracker
      |  evaluates filters
      |  FileWorkItem (bounded channel)               ReadyDirectory (bounded channel)
      v                                                       v
file worker pool                                       directory worker pool
(semaphore-gated, up to                                (fixed, small: 4 threads --
 --workers ceiling)                                     directory removal is cheap and
      |                                                 must not be starved by heavy
      | tracker.complete_child(parent)                  file-delete concurrency)
      +-----------------------------------------------------+
                              |
                    DirectoryTracker (crate::directory_tracker)
```

Backpressure is structural, not a separate mechanism: if delete workers
fall behind, the bounded `FileWorkItem` channel fills, which blocks the
coordinator's send, which stops it draining the `EnumEvent` channel, which
fills *that* channel and blocks the enumerator. Memory use is bounded by
channel capacities (a small constant) plus the number of directories
*currently in flight* (`DirectoryTracker`, a `DashMap` keyed by directory,
entries removed once a directory is finalised) -- never by total file
count. This is verified directly: `directory_tracker::tests` assert the
tracker's size reflects in-flight directories, not total files processed.

Directories are deleted only once `DirectoryTracker` observes that a
directory's enumeration is complete *and* every child (file, symlink, or
nested directory) has resolved -- see `directory_tracker.rs` module docs
for the exact bookkeeping, and `pipeline::process_one`/`dir_worker_loop`
for how a resolution outcome (deleted / retained / failed) propagates to
the parent.

### Separate concurrency pools

Directory removal runs in a small fixed pool (4 threads) independent of
the file-delete worker pool, so a large ordinary delete workload can never
starve directory cleanup, and vice versa -- directly satisfying "a
permission-heavy workload must not stall ordinary deletions" (the same
principle extended to directory work). Remediation and lock-recovery calls
happen *inline* within a file worker's own slot (`pipeline::process_one`)
rather than a separate pool in the initial implementation: they are always
triggered by that worker's own failed attempt, so routing them through a
separate queue would only add latency without protecting other workers,
which are already isolated by the semaphore gate. If profiling later shows
remediation-heavy files monopolising worker slots for pathological inputs,
splitting remediation into its own bounded pool is a contained follow-up
change, not a redesign.

### Directory-worker shutdown

A subtlety worth recording: `DirectoryTracker` is `Clone`, and every
directory worker holds a clone (to call `complete_child`/`forget`) whose
internal sender targets the *same* channel that worker reads from. This
means the ready-directory channel can never close on its own (a worker
would have to drop its own sender to unblock itself). Shutdown is
therefore an explicit shared `AtomicBool` flag, set by the coordinator once
the root directory's own completion signal fires, polled by directory
workers via `recv_timeout` -- not channel-closure detection. See the
`DIR_WORKER_POLL_INTERVAL` doc comment in `pipeline.rs`.

## Consequences

- Deletion demonstrably starts before enumeration finishes for any tree
  with more than one directory level (the coordinator dispatches file work
  items as soon as they're discovered, not after the walk completes).
- The pipeline is engine-agnostic: `cursdel_core::pipeline::run` takes `&dyn
  PlatformEngine` and has zero platform-specific code, so it is fully unit-
  and integration-testable without any real platform engine (see the fake
  `DirLister` in `walk.rs` tests).
- Cancellation (Ctrl+C) is cooperative: in-flight native delete syscalls
  are never forcibly interrupted (there is no safe way to do that); instead
  the enumerator stops discovering new work promptly (checked at each
  directory boundary) and flushes `DirectoryComplete` for every directory
  still open on its stack, guaranteeing the pipeline still reaches a
  consistent terminal state and reports accurate partial results rather
  than hanging. See `walk::tests::cancellation_flushes_remaining_open_directories`.
