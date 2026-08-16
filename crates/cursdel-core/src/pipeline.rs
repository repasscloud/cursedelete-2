//! Orchestration: wires target validation, the streaming enumerator, the
//! bounded work queue, the adaptive worker pool, the directory-completion
//! tracker, filters/retention, and remediation/lock recovery into one
//! `run()` call that produces a [`Summary`].
//!
//! See `docs/adr/0002-streaming-pipeline.md` for the architecture this
//! implements.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::adaptive::{AdaptiveController, AdaptiveDecision};
use crate::directory_tracker::{DirectoryTracker, ReadyDirectory};
use crate::engine::{
    CancelToken, DeleteOptions, DeleteOutcome, DirIdAllocator, EnumEvent, EnumSink, LockResolution,
    PlatformEngine, RemediationOutcome,
};
use crate::entry::{DirId, DiscoveredEntry, EntryKind, EntryTimes};
use crate::error::{DeleteFailure, FailureCategory, TargetError};
use crate::filter::{FilterDecision, FilterSet};
use crate::metrics::{OperationMetrics, WindowSampler};
use crate::options::{OperationOptions, WorkerPolicy};
use crate::report::Summary;
use crate::target::{self, CanonicalTarget};

const ENUM_CHANNEL_CAP: usize = 4096;
const FILE_CHANNEL_CAP: usize = 4096;
const DIR_CHANNEL_CAP: usize = 4096;
const DIR_WORKER_COUNT: usize = 4;
/// Absolute ceiling on how many file-worker OS threads `--workers auto`
/// will ever pre-spawn, regardless of CPU count. Manual `--workers <n>`
/// is not bound by this -- an operator who explicitly asks for more
/// concurrency gets exactly what they asked for.
const AUTO_MAX_WORKERS_CEILING: usize = 256;
/// How many file-worker threads (per available CPU) `--workers auto`
/// pre-spawns as its ceiling. Every one of these threads is spawned
/// up front (see `docs/adr/0003-adaptive-workers.md`) even though the
/// adaptive controller may never grow concurrency anywhere near it, so
/// this must stay proportionate to real hardware rather than a large
/// flat constant: benchmarking a small tree (a few hundred files) showed
/// a fixed 256-thread pre-spawn taking multiple seconds of pure thread-
/// creation overhead on a 10-core machine, several times slower than
/// `rm -rf` on the same tree purely from that fixed cost -- see
/// `docs/BENCHMARKS.md`.
const AUTO_MAX_WORKERS_PER_CPU: usize = 16;
const SAMPLE_INTERVAL: Duration = Duration::from_millis(400);
const MAX_STORED_FAILURES: usize = 10_000;

#[derive(Default)]
struct FailureLog {
    failures: Mutex<Vec<DeleteFailure>>,
    truncated: AtomicBool,
}

impl FailureLog {
    fn push(&self, failure: DeleteFailure) {
        let mut guard = self.failures.lock().unwrap();
        if guard.len() < MAX_STORED_FAILURES {
            guard.push(failure);
        } else {
            self.truncated.store(true, Ordering::Relaxed);
        }
    }

    fn into_parts(self) -> (Vec<DeleteFailure>, bool) {
        (
            self.failures.into_inner().unwrap(),
            self.truncated.load(Ordering::Relaxed),
        )
    }
}

struct FileWorkItem {
    path: PathBuf,
    parent_dir_id: DirId,
    size: u64,
    /// True for a directory-type reparse point (Windows junction /
    /// directory symlink), which must be removed via the directory-style
    /// native call even though it is treated as a leaf for tracker
    /// purposes.
    dispatch_as_dir: bool,
}

/// How long a directory worker waits on an empty ready-queue before
/// re-checking the shutdown flag. Directory workers cannot rely on
/// channel-closure to know when to stop: every worker holds a
/// `DirectoryTracker` clone (needed to call `complete_child`/`forget`),
/// and that clone carries its own sender to the very channel the worker
/// reads from, so the channel can never close on its own. A shared
/// shutdown flag, polled on each timeout, avoids that structural
/// deadlock.
const DIR_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Run one deletion operation to completion (or until cancelled).
///
/// `filters` must already be built (glob compilation can fail, and that is
/// a CLI usage error the caller should surface before committing to a
/// destructive operation, not something buried in the middle of a run).
pub fn run(
    engine: &dyn PlatformEngine,
    options: &OperationOptions,
    filters: &FilterSet,
    cancel: CancelToken,
) -> Result<Summary, TargetError> {
    let start = Instant::now();
    let target = target::validate_target(&options.target)?;
    let delete_opts = options.delete_options();
    let metrics = OperationMetrics::default();
    let failure_log = FailureLog::default();

    if !target.is_dir {
        run_single_object(
            engine,
            &target,
            filters,
            options.dry_run,
            delete_opts,
            &metrics,
            &failure_log,
        );
        return Ok(build_summary(
            options,
            metrics,
            failure_log,
            start.elapsed(),
            1,
            false,
        ));
    }

    let (enum_tx, enum_rx) = crossbeam_channel::bounded::<EnumEvent>(ENUM_CHANNEL_CAP);
    let (file_tx, file_rx) = crossbeam_channel::bounded::<FileWorkItem>(FILE_CHANNEL_CAP);
    let (dir_tx, dir_rx) = crossbeam_channel::bounded::<ReadyDirectory>(DIR_CHANNEL_CAP);
    let (root_done_tx, root_done_rx) = crossbeam_channel::bounded::<()>(1);
    let dir_workers_done = Arc::new(AtomicBool::new(false));

    let tracker = DirectoryTracker::new(dir_tx);
    tracker.register_directory(DirId(0), None, target.canonical.clone());
    // The root itself is never emitted as a discovered EnumEvent (it's
    // the walk's starting point, not a child of anything within this
    // operation), so it would otherwise be absent from `dirs_scanned`
    // while still counting toward `dirs_deleted`/`dirs_retained` --
    // counted here so the report's "scanned" and "deleted" directory
    // totals stay coherent with each other.
    metrics.dirs_scanned.fetch_add(1, Ordering::Relaxed);

    let preserve_root = options.preserve_root();

    let (worker_min, worker_max, worker_initial) = match options.workers {
        WorkerPolicy::Fixed(n) => (n.max(1), n.max(1), n.max(1)),
        WorkerPolicy::Auto => {
            let cpu = num_cpus::get().max(1);
            let initial = cpu.clamp(4, 16);
            let ceiling = (cpu * AUTO_MAX_WORKERS_PER_CPU).clamp(32, AUTO_MAX_WORKERS_CEILING);
            (1, ceiling, initial)
        }
    };

    let sem = crate::sync::ResizableSemaphore::new(worker_initial);
    let sampler = WindowSampler::default();
    let controller = match options.workers {
        WorkerPolicy::Fixed(_) => None,
        WorkerPolicy::Auto => Some(Mutex::new(AdaptiveController::new(
            worker_min,
            worker_max,
            worker_initial,
        ))),
    };
    let final_workers = AtomicUsize::new(worker_initial);
    let dry_run = options.dry_run;

    std::thread::scope(|scope| {
        for _ in 0..DIR_WORKER_COUNT {
            let tracker = tracker.clone();
            let dir_rx = dir_rx.clone();
            let metrics = &metrics;
            let failure_log = &failure_log;
            let root_done_tx = root_done_tx.clone();
            let dir_workers_done = dir_workers_done.clone();
            let cancel = cancel.clone();
            scope.spawn(move || {
                dir_worker_loop(
                    engine,
                    &dir_rx,
                    &tracker,
                    metrics,
                    failure_log,
                    delete_opts,
                    dry_run,
                    preserve_root,
                    &cancel,
                    &root_done_tx,
                    &dir_workers_done,
                );
            });
        }

        for _ in 0..worker_max {
            let tracker = tracker.clone();
            let file_rx = file_rx.clone();
            let metrics = &metrics;
            let failure_log = &failure_log;
            let sampler = &sampler;
            let sem = &sem;
            scope.spawn(move || {
                file_worker_loop(
                    engine,
                    &file_rx,
                    sem,
                    &tracker,
                    metrics,
                    sampler,
                    failure_log,
                    delete_opts,
                );
            });
        }

        let dir_id_alloc = std::sync::Arc::new(DirIdAllocator::new());
        let sink = EnumSink::new(enum_tx.clone(), dir_id_alloc);
        drop(enum_tx);
        let enum_cancel = cancel.clone();
        let enum_metrics = &metrics;
        let enum_failure_log = &failure_log;
        let target_ref = &target;
        scope.spawn(move || {
            if let Err(e) = engine.enumerate(target_ref, &sink, &enum_cancel) {
                if !matches!(e, crate::engine::EnumError::Cancelled) {
                    enum_metrics.failures.fetch_add(1, Ordering::Relaxed);
                    enum_failure_log.push(DeleteFailure {
                        path: target_ref.canonical.clone(),
                        is_directory: true,
                        category: FailureCategory::Io,
                        message: e.to_string(),
                        os_error_code: None,
                    });
                }
            }
        });

        coordinator_loop(
            &enum_rx,
            &tracker,
            filters,
            &file_tx,
            &metrics,
            &failure_log,
            dry_run,
            &cancel,
        );
        drop(file_tx);

        loop {
            match root_done_rx.recv_timeout(SAMPLE_INTERVAL) {
                Ok(()) => break,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if let Some(controller) = &controller {
                        let stats = sampler.take_window();
                        let mut c = controller.lock().unwrap();
                        if let AdaptiveDecision::Increase(n) | AdaptiveDecision::Decrease(n) =
                            c.on_sample(&stats)
                        {
                            sem.set_capacity(n);
                            final_workers.store(n, Ordering::Relaxed);
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }

        dir_workers_done.store(true, Ordering::SeqCst);
    });

    let workers_final = match options.workers {
        WorkerPolicy::Fixed(n) => n,
        WorkerPolicy::Auto => final_workers.load(Ordering::Relaxed),
    };

    Ok(build_summary(
        options,
        metrics,
        failure_log,
        start.elapsed(),
        workers_final,
        cancel.is_cancelled(),
    ))
}

#[allow(clippy::too_many_arguments)]
fn coordinator_loop(
    enum_rx: &Receiver<EnumEvent>,
    tracker: &DirectoryTracker,
    filters: &FilterSet,
    file_tx: &Sender<FileWorkItem>,
    metrics: &OperationMetrics,
    failure_log: &FailureLog,
    dry_run: bool,
    cancel: &CancelToken,
) {
    for event in enum_rx.iter() {
        match event {
            EnumEvent::Entry(entry) => {
                handle_entry(entry, tracker, filters, file_tx, metrics, dry_run, cancel)
            }
            EnumEvent::DirectoryComplete { dir_id } => {
                tracker.mark_enumeration_done(dir_id);
            }
            EnumEvent::Error { path, message } => {
                metrics.failures.fetch_add(1, Ordering::Relaxed);
                failure_log.push(DeleteFailure {
                    path,
                    is_directory: true,
                    category: FailureCategory::Io,
                    message,
                    os_error_code: None,
                });
            }
        }
    }
}

fn handle_entry(
    entry: DiscoveredEntry,
    tracker: &DirectoryTracker,
    filters: &FilterSet,
    file_tx: &Sender<FileWorkItem>,
    metrics: &OperationMetrics,
    dry_run: bool,
    cancel: &CancelToken,
) {
    if entry.is_dir() {
        let dir_id = entry
            .dir_id
            .expect("Directory entries always carry a dir_id");
        metrics.dirs_scanned.fetch_add(1, Ordering::Relaxed);
        tracker.register_directory(dir_id, entry.parent_dir_id, entry.path);
        return;
    }

    let parent = entry
        .parent_dir_id
        .expect("non-root leaf entries always have a parent");
    tracker.register_leaf_child(parent);
    metrics.files_scanned.fetch_add(1, Ordering::Relaxed);

    if cancel.is_cancelled() {
        metrics.files_retained.fetch_add(1, Ordering::Relaxed);
        tracker.complete_child(parent, true);
        return;
    }

    match filters.evaluate_file(&entry) {
        FilterDecision::Retain(_) => {
            metrics.files_retained.fetch_add(1, Ordering::Relaxed);
            tracker.complete_child(parent, true);
        }
        FilterDecision::Delete => {
            if dry_run {
                metrics.files_deleted.fetch_add(1, Ordering::Relaxed);
                metrics
                    .bytes_deleted
                    .fetch_add(entry.size, Ordering::Relaxed);
                tracker.complete_child(parent, false);
            } else {
                let dispatch_as_dir = matches!(
                    entry.kind,
                    EntryKind::Symlink {
                        reparse_is_dir: true
                    }
                );
                let item = FileWorkItem {
                    path: entry.path,
                    parent_dir_id: parent,
                    size: entry.size,
                    dispatch_as_dir,
                };
                if file_tx.send(item).is_err() {
                    tracker.complete_child(parent, true);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn file_worker_loop(
    engine: &dyn PlatformEngine,
    file_rx: &Receiver<FileWorkItem>,
    sem: &crate::sync::ResizableSemaphore,
    tracker: &DirectoryTracker,
    metrics: &OperationMetrics,
    sampler: &WindowSampler,
    failure_log: &FailureLog,
    delete_opts: DeleteOptions,
) {
    while let Ok(item) = file_rx.recv() {
        sem.acquire();
        let attempt = process_one(engine, &item, delete_opts, metrics);
        sem.release();

        let still_present = match &attempt.outcome {
            DeleteOutcome::Deleted | DeleteOutcome::AlreadyGone => {
                metrics.files_deleted.fetch_add(1, Ordering::Relaxed);
                metrics
                    .bytes_deleted
                    .fetch_add(item.size, Ordering::Relaxed);
                sampler.record_success();
                // `--verbose` per-file logging. Cheap when disabled by
                // design: with no subscriber active (or a level filter
                // excluding `debug`), `tracing`'s macros short-circuit
                // before formatting or evaluating their arguments, so
                // this does not cost the normal fast path anything --
                // see docs/adr/0002-streaming-pipeline.md's performance
                // principles and `cursdel-cli::main`'s subscriber setup.
                tracing::debug!(path = %item.path.display(), bytes = item.size, "deleted");
                false
            }
            DeleteOutcome::Failed(failure) => {
                metrics.failures.fetch_add(1, Ordering::Relaxed);
                sampler.record_error();
                tracing::debug!(
                    path = %item.path.display(),
                    category = %failure.category,
                    message = %failure.message,
                    "delete failed"
                );
                failure_log.push(failure.clone());
                true
            }
        };

        tracker.complete_child(item.parent_dir_id, still_present);
    }
}

fn base_delete(
    engine: &dyn PlatformEngine,
    path: &Path,
    dispatch_as_dir: bool,
    opts: DeleteOptions,
) -> crate::engine::DeleteAttempt {
    if dispatch_as_dir {
        engine.delete_dir(path, opts)
    } else {
        engine.delete_file(path, opts)
    }
}

fn is_remediable(category: FailureCategory) -> bool {
    matches!(
        category,
        FailureCategory::AccessDenied | FailureCategory::ReparsePointRefused
    )
}

fn process_one(
    engine: &dyn PlatformEngine,
    item: &FileWorkItem,
    opts: DeleteOptions,
    metrics: &OperationMetrics,
) -> crate::engine::DeleteAttempt {
    let mut attempt = base_delete(engine, &item.path, item.dispatch_as_dir, opts);

    if let DeleteOutcome::Failed(ref failure) = attempt.outcome {
        if opts.allow_remediation && is_remediable(failure.category) {
            metrics.remediation_attempts.fetch_add(1, Ordering::Relaxed);
            if let RemediationOutcome::Applied =
                engine.remediate(&item.path, item.dispatch_as_dir, failure, opts)
            {
                metrics
                    .remediation_successes
                    .fetch_add(1, Ordering::Relaxed);
                metrics.retries.fetch_add(1, Ordering::Relaxed);
                attempt = base_delete(engine, &item.path, item.dispatch_as_dir, opts);
            }
        }
    }

    if let DeleteOutcome::Failed(ref failure) = attempt.outcome {
        if failure.category == FailureCategory::SharingViolation && opts.kill_locks {
            if let LockResolution::Resolved = engine.resolve_local_lock(&item.path, opts) {
                metrics.local_locks_resolved.fetch_add(1, Ordering::Relaxed);
                metrics.retries.fetch_add(1, Ordering::Relaxed);
                attempt = base_delete(engine, &item.path, item.dispatch_as_dir, opts);
            }
        }
    }

    if let DeleteOutcome::Failed(ref failure) = attempt.outcome {
        if failure.category == FailureCategory::SharingViolation && opts.close_remote_locks {
            match engine.resolve_remote_lock(&item.path, opts) {
                LockResolution::Resolved => {
                    metrics
                        .remote_locks_resolved
                        .fetch_add(1, Ordering::Relaxed);
                    metrics.retries.fetch_add(1, Ordering::Relaxed);
                    attempt = base_delete(engine, &item.path, item.dispatch_as_dir, opts);
                }
                LockResolution::Unsupported(msg) => {
                    attempt = crate::engine::DeleteAttempt::failed(DeleteFailure {
                        path: item.path.clone(),
                        is_directory: item.dispatch_as_dir,
                        category: FailureCategory::RemoteLockUnsupported,
                        message: msg,
                        os_error_code: None,
                    });
                }
                LockResolution::Failed(msg) => {
                    attempt = crate::engine::DeleteAttempt::failed(DeleteFailure {
                        path: item.path.clone(),
                        is_directory: item.dispatch_as_dir,
                        category: FailureCategory::RemoteLockFailed,
                        message: msg,
                        os_error_code: None,
                    });
                }
            }
        }
    }

    attempt
}

#[allow(clippy::too_many_arguments)]
fn dir_worker_loop(
    engine: &dyn PlatformEngine,
    rx: &Receiver<ReadyDirectory>,
    tracker: &DirectoryTracker,
    metrics: &OperationMetrics,
    failure_log: &FailureLog,
    delete_opts: DeleteOptions,
    dry_run: bool,
    preserve_root: bool,
    cancel: &CancelToken,
    root_done_tx: &Sender<()>,
    shutdown: &AtomicBool,
) {
    loop {
        let ready = match rx.recv_timeout(DIR_WORKER_POLL_INTERVAL) {
            Ok(r) => r,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if shutdown.load(Ordering::SeqCst) && rx.is_empty() {
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let is_root = ready.parent.is_none();

        if is_root && preserve_root {
            tracker.forget(ready.dir_id);
            let _ = root_done_tx.send(());
            continue;
        }

        let still_present = if dry_run {
            if ready.simulated_non_empty {
                metrics.dirs_retained.fetch_add(1, Ordering::Relaxed);
            } else {
                metrics.dirs_deleted.fetch_add(1, Ordering::Relaxed);
            }
            ready.simulated_non_empty
        } else {
            let attempt = engine.delete_dir(&ready.path, delete_opts);
            match &attempt.outcome {
                DeleteOutcome::Deleted | DeleteOutcome::AlreadyGone => {
                    metrics.dirs_deleted.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(path = %ready.path.display(), "directory removed");
                    false
                }
                DeleteOutcome::Failed(failure)
                    if failure.category == FailureCategory::NotEmpty
                        && (preserve_root || cancel.is_cancelled()) =>
                {
                    // Expected, not an operational failure: either
                    // retention/filtering deliberately left children
                    // behind, or the operation was interrupted before it
                    // could finish and objects were necessarily left
                    // behind as a result.
                    metrics.dirs_retained.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(path = %ready.path.display(), "directory retained (not empty)");
                    true
                }
                DeleteOutcome::Failed(failure) => {
                    metrics.failures.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(
                        path = %ready.path.display(),
                        category = %failure.category,
                        message = %failure.message,
                        "directory removal failed"
                    );
                    failure_log.push(failure.clone());
                    true
                }
            }
        };

        tracker.forget(ready.dir_id);
        if let Some(parent) = ready.parent {
            tracker.complete_child(parent, still_present);
        } else {
            let _ = root_done_tx.send(());
        }
    }
}

fn run_single_object(
    engine: &dyn PlatformEngine,
    target: &CanonicalTarget,
    filters: &FilterSet,
    dry_run: bool,
    opts: DeleteOptions,
    metrics: &OperationMetrics,
    failure_log: &FailureLog,
) {
    let meta = match std::fs::symlink_metadata(&target.requested) {
        Ok(m) => m,
        Err(e) => {
            metrics.failures.fetch_add(1, Ordering::Relaxed);
            failure_log.push(DeleteFailure {
                path: target.requested.clone(),
                is_directory: false,
                category: FailureCategory::NotFound,
                message: e.to_string(),
                os_error_code: None,
            });
            return;
        }
    };

    let size = if meta.is_dir() { 0 } else { meta.len() };
    metrics.files_scanned.fetch_add(1, Ordering::Relaxed);

    let entry = DiscoveredEntry {
        path: target.requested.clone(),
        dir_id: None,
        parent_dir_id: None,
        kind: EntryKind::File,
        size,
        times: EntryTimes {
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            accessed: meta.accessed().ok(),
        },
        readonly: meta.permissions().readonly(),
    };

    if let FilterDecision::Retain(_) = filters.evaluate_file(&entry) {
        metrics.files_retained.fetch_add(1, Ordering::Relaxed);
        return;
    }

    if dry_run {
        metrics.files_deleted.fetch_add(1, Ordering::Relaxed);
        metrics.bytes_deleted.fetch_add(size, Ordering::Relaxed);
        return;
    }

    let dispatch_as_dir = meta.is_dir();
    let item = FileWorkItem {
        path: target.requested.clone(),
        parent_dir_id: DirId(0),
        size,
        dispatch_as_dir,
    };
    let attempt = process_one(engine, &item, opts, metrics);
    match attempt.outcome {
        DeleteOutcome::Deleted | DeleteOutcome::AlreadyGone => {
            metrics.files_deleted.fetch_add(1, Ordering::Relaxed);
            metrics.bytes_deleted.fetch_add(size, Ordering::Relaxed);
        }
        DeleteOutcome::Failed(failure) => {
            metrics.failures.fetch_add(1, Ordering::Relaxed);
            failure_log.push(failure);
        }
    }
}

fn build_summary(
    options: &OperationOptions,
    metrics: OperationMetrics,
    failure_log: FailureLog,
    elapsed: Duration,
    workers_final: usize,
    interrupted: bool,
) -> Summary {
    let (failures, failures_truncated) = failure_log.into_parts();
    Summary {
        target: options.target.clone(),
        mode: options.mode,
        dry_run: options.dry_run,
        workers_requested: options.workers,
        workers_final,
        metrics: metrics.snapshot(),
        elapsed,
        failures,
        failures_truncated,
        interrupted,
        acl_repair_enabled: options.mode.allows_remediation(),
        kill_locks_enabled: options.effective_kill_locks(),
        remote_locks_enabled: options.close_remote_locks,
    }
}
