//! Two independent counter sets: [`OperationMetrics`], which accumulates
//! for the whole run and feeds the final report, and [`WindowSampler`],
//! which resets every sampling window and feeds
//! `crate::adaptive::AdaptiveController`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::adaptive::WindowStats;

#[derive(Default)]
pub struct OperationMetrics {
    pub files_scanned: AtomicU64,
    pub dirs_scanned: AtomicU64,
    pub files_deleted: AtomicU64,
    pub dirs_deleted: AtomicU64,
    pub files_retained: AtomicU64,
    pub dirs_retained: AtomicU64,
    pub bytes_deleted: AtomicU64,
    pub failures: AtomicU64,
    pub remediation_attempts: AtomicU64,
    pub remediation_successes: AtomicU64,
    pub local_locks_resolved: AtomicU64,
    pub remote_locks_resolved: AtomicU64,
    pub retries: AtomicU64,
}

impl OperationMetrics {
    pub fn snapshot(&self) -> OperationMetricsSnapshot {
        OperationMetricsSnapshot {
            files_scanned: self.files_scanned.load(Ordering::Relaxed),
            dirs_scanned: self.dirs_scanned.load(Ordering::Relaxed),
            files_deleted: self.files_deleted.load(Ordering::Relaxed),
            dirs_deleted: self.dirs_deleted.load(Ordering::Relaxed),
            files_retained: self.files_retained.load(Ordering::Relaxed),
            dirs_retained: self.dirs_retained.load(Ordering::Relaxed),
            bytes_deleted: self.bytes_deleted.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            remediation_attempts: self.remediation_attempts.load(Ordering::Relaxed),
            remediation_successes: self.remediation_successes.load(Ordering::Relaxed),
            local_locks_resolved: self.local_locks_resolved.load(Ordering::Relaxed),
            remote_locks_resolved: self.remote_locks_resolved.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationMetricsSnapshot {
    pub files_scanned: u64,
    pub dirs_scanned: u64,
    pub files_deleted: u64,
    pub dirs_deleted: u64,
    pub files_retained: u64,
    pub dirs_retained: u64,
    pub bytes_deleted: u64,
    pub failures: u64,
    pub remediation_attempts: u64,
    pub remediation_successes: u64,
    pub local_locks_resolved: u64,
    pub remote_locks_resolved: u64,
    pub retries: u64,
}

/// Short-window counters feeding the adaptive controller. Reset every
/// sampling interval so `take_window` reflects only recent behaviour.
pub struct WindowSampler {
    completed: AtomicU64,
    errors: AtomicU64,
    window_start: std::sync::Mutex<Instant>,
}

impl Default for WindowSampler {
    fn default() -> Self {
        Self {
            completed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            window_start: std::sync::Mutex::new(Instant::now()),
        }
    }
}

impl WindowSampler {
    pub fn record_success(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Atomically drain the window's counters and return the stats since
    /// the last call (or since construction).
    pub fn take_window(&self) -> WindowStats {
        let completed = self.completed.swap(0, Ordering::AcqRel);
        let errors = self.errors.swap(0, Ordering::AcqRel);
        let mut start = self.window_start.lock().unwrap();
        let elapsed = start.elapsed();
        *start = Instant::now();
        WindowStats {
            completed,
            errors,
            wall_elapsed: elapsed.max(Duration::from_millis(1)),
        }
    }
}
