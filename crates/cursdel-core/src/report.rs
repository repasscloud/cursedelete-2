//! Human-readable and machine-readable (`--json`) operation summaries, and
//! the mapping from a finished operation to a stable [`ExitCode`].
//!
//! The JSON shape here is a public contract (`docs/JSON_OUTPUT.md`) --
//! field names and types must not change without a documented schema
//! version bump.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::error::DeleteFailure;
use crate::exit_code::ExitCode;
use crate::metrics::OperationMetricsSnapshot;
use crate::options::{Mode, WorkerPolicy};

/// Schema version for `--json` output. Bump on any breaking change to the
/// shape of [`JsonReport`].
pub const JSON_SCHEMA_VERSION: u32 = 1;

pub struct Summary {
    pub target: PathBuf,
    pub mode: Mode,
    pub dry_run: bool,
    pub workers_requested: WorkerPolicy,
    pub workers_final: usize,
    pub metrics: OperationMetricsSnapshot,
    pub elapsed: Duration,
    pub failures: Vec<DeleteFailure>,
    /// True if more failures occurred than the detailed `failures` list
    /// retains (see `MAX_STORED_FAILURES` in `crate::pipeline`). The
    /// failure *count* in `metrics` is always exact regardless of this
    /// flag; only the per-object detail is capped, to keep memory bounded
    /// even in a pathological failure storm.
    pub failures_truncated: bool,
    pub interrupted: bool,
    pub acl_repair_enabled: bool,
    pub kill_locks_enabled: bool,
    pub remote_locks_enabled: bool,
}

impl Summary {
    /// Classification is based on the (possibly truncated, see
    /// `failures_truncated`) sample of failures actually retained. In the
    /// pathological case of more than `MAX_STORED_FAILURES` failures whose
    /// categories are not uniform across the run, the most specific exit
    /// code (remote/local lock, privilege) could be missed in favour of
    /// the generic [`ExitCode::CompletedWithFailures`] -- accepted as a
    /// rare edge case of an already-rare failure storm; the failure
    /// *count* itself is never approximate.
    pub fn exit_code(&self) -> ExitCode {
        if self.interrupted {
            return ExitCode::Interrupted;
        }
        if !self.failures.is_empty() {
            let any_remote_lock_failure = self.failures.iter().any(|f| {
                matches!(
                    f.category,
                    crate::error::FailureCategory::RemoteLockFailed
                        | crate::error::FailureCategory::RemoteLockUnsupported
                )
            });
            if any_remote_lock_failure && self.remote_locks_enabled {
                return ExitCode::RemoteLockHandlingFailed;
            }
            let any_local_lock_failure = self
                .failures
                .iter()
                .any(|f| f.category == crate::error::FailureCategory::LocalLockUnresolved);
            if any_local_lock_failure && self.kill_locks_enabled {
                return ExitCode::LockResolutionFailed;
            }
            // Remediation was explicitly requested (--force/--destroy),
            // attempted at least once, and never once succeeded: the most
            // likely explanation is that the executing security context
            // simply doesn't hold the privilege remediation would need,
            // not that individual objects happened to fail for unrelated
            // reasons. Distinct from the general CompletedWithFailures
            // case so automation can tell "we don't have permission to
            // fix this at all" apart from "most things worked, a few
            // objects failed."
            if self.acl_repair_enabled
                && self.metrics.remediation_attempts > 0
                && self.metrics.remediation_successes == 0
            {
                return ExitCode::PrivilegeRequirementNotSatisfied;
            }
            return ExitCode::CompletedWithFailures;
        }
        ExitCode::Success
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let m = &self.metrics;

        out.push_str("CurseDelete 2\n\n");
        out.push_str(&format!("Target:       {}\n", self.target.display()));
        out.push_str(&format!(
            "Mode:         {}{}\n",
            self.mode.as_str(),
            if self.dry_run { " (dry-run)" } else { "" }
        ));
        out.push_str(&format!(
            "Workers:      {}\n",
            format_workers(self.workers_requested, self.workers_final)
        ));
        out.push_str(&format!(
            "ACL repair:   {}\n",
            enabled_str(self.acl_repair_enabled)
        ));
        out.push_str(&format!(
            "Kill locks:   {}\n",
            enabled_str(self.kill_locks_enabled)
        ));
        out.push_str(&format!(
            "Remote locks: {}\n",
            enabled_str(self.remote_locks_enabled)
        ));
        out.push('\n');

        if self.dry_run {
            out.push_str(&format!(
                "Files scanned:       {}\n",
                format_count(m.files_scanned)
            ));
            out.push_str(&format!(
                "Directories scanned: {}\n",
                format_count(m.dirs_scanned)
            ));
            out.push('\n');
            out.push_str("Would delete:\n");
            out.push_str(&format!(
                "  Files:             {}\n",
                format_count(m.files_deleted)
            ));
            out.push_str(&format!(
                "  Directories:       {}\n",
                format_count(m.dirs_deleted)
            ));
            out.push_str(&format!(
                "  Data:              {}\n",
                format_bytes_human(m.bytes_deleted)
            ));
            out.push('\n');
            out.push_str("Would retain:\n");
            out.push_str(&format!(
                "  Files:             {}\n",
                format_count(m.files_retained)
            ));
            out.push_str(&format!(
                "  Directories:       {}\n",
                format_count(m.dirs_retained)
            ));
            out.push('\n');
            if !self.failures.is_empty() {
                out.push_str(&format!(
                    "Scan failures:       {}\n\n",
                    format_count(self.failures.len() as u64)
                ));
            }
            out.push_str("No files were modified.\n");
            return out;
        }

        out.push_str(&format!(
            "Files:          {}\n",
            format_count(m.files_deleted)
        ));
        out.push_str(&format!(
            "Directories:    {}\n",
            format_count(m.dirs_deleted)
        ));
        if m.files_retained > 0 || m.dirs_retained > 0 {
            out.push_str(&format!(
                "Retained:       {} files, {} directories\n",
                format_count(m.files_retained),
                format_count(m.dirs_retained)
            ));
        }
        out.push_str(&format!(
            "Deleted:        {}\n",
            format_bytes_human(m.bytes_deleted)
        ));
        out.push_str(&format!(
            "Failures:       {}{}\n",
            format_count(m.failures),
            if self.failures_truncated {
                " (detail truncated, see --log)"
            } else {
                ""
            }
        ));
        out.push_str(&format!(
            "Elapsed:        {}\n",
            format_elapsed(self.elapsed)
        ));
        out.push_str(&format!(
            "Rate:           {}\n",
            format_rate(m.files_deleted, self.elapsed)
        ));
        out.push('\n');

        if self.interrupted {
            out.push_str("Interrupted -- partial results reported above.\n");
        } else if !self.failures.is_empty() {
            out.push_str("Completed with failures.\n");
        } else {
            out.push_str("Complete.\n");
        }

        out
    }

    pub fn to_json(&self) -> JsonReport {
        JsonReport {
            schema_version: JSON_SCHEMA_VERSION,
            target: self.target.display().to_string(),
            mode: self.mode.as_str().to_string(),
            dry_run: self.dry_run,
            workers_requested: match self.workers_requested {
                WorkerPolicy::Auto => "auto".to_string(),
                WorkerPolicy::Fixed(n) => n.to_string(),
            },
            workers_final: self.workers_final,
            acl_repair_enabled: self.acl_repair_enabled,
            kill_locks_enabled: self.kill_locks_enabled,
            remote_locks_enabled: self.remote_locks_enabled,
            elapsed_ms: self.elapsed.as_millis() as u64,
            interrupted: self.interrupted,
            metrics: self.metrics,
            failures: self.failures.clone(),
            failures_truncated: self.failures_truncated,
            exit_code: self.exit_code().code(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonReport {
    pub schema_version: u32,
    pub target: String,
    pub mode: String,
    pub dry_run: bool,
    pub workers_requested: String,
    pub workers_final: usize,
    pub acl_repair_enabled: bool,
    pub kill_locks_enabled: bool,
    pub remote_locks_enabled: bool,
    pub elapsed_ms: u64,
    pub interrupted: bool,
    pub metrics: OperationMetricsSnapshot,
    pub failures: Vec<DeleteFailure>,
    pub failures_truncated: bool,
    pub exit_code: i32,
}

fn enabled_str(b: bool) -> &'static str {
    if b {
        "enabled"
    } else {
        "disabled"
    }
}

fn format_workers(requested: WorkerPolicy, actual: usize) -> String {
    match requested {
        WorkerPolicy::Auto => format!("auto -> {actual}"),
        WorkerPolicy::Fixed(n) => n.to_string(),
    }
}

/// Formats a count with thousands separators, e.g. `4_821_992` -> `4,821,992`.
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Human-readable byte size. Uses binary (1024-based) magnitudes, labelled
/// with the conventional (non-"i") unit names to match the product's
/// documented output style -- e.g. `891.40 GB` means 891.40 * 1024^3
/// bytes, not 891.40 * 10^9. This mirrors how most desktop file managers
/// label sizes and is intentional, not an SI/IEC mixup.
pub fn format_bytes_human(bytes: u64) -> String {
    const UNITS: [(&str, f64); 5] = [
        ("TB", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("GB", 1024.0 * 1024.0 * 1024.0),
        ("MB", 1024.0 * 1024.0),
        ("KB", 1024.0),
        ("B", 1.0),
    ];
    for (name, threshold) in UNITS {
        if bytes as f64 >= threshold || name == "B" {
            return format!("{:.1} {name}", bytes as f64 / threshold);
        }
    }
    format!("{bytes} B")
}

pub fn format_elapsed(d: Duration) -> String {
    let total_secs = d.as_secs();
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn format_rate(files_deleted: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return "n/a".to_string();
    }
    let rate = files_deleted as f64 / secs;
    format!("{} files/sec", format_count(rate.round() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(4_821_992), "4,821,992");
    }

    #[test]
    fn elapsed_formats_hh_mm_ss() {
        assert_eq!(format_elapsed(Duration::from_secs(221)), "00:03:41");
        assert_eq!(format_elapsed(Duration::from_secs(3661)), "01:01:01");
    }

    #[test]
    fn byte_formatting_picks_appropriate_unit() {
        assert_eq!(format_bytes_human(500), "500.0 B");
        assert_eq!(format_bytes_human(2048), "2.0 KB");
    }

    fn base_summary() -> Summary {
        Summary {
            target: PathBuf::from("/tmp/x"),
            mode: Mode::Normal,
            dry_run: false,
            workers_requested: WorkerPolicy::Auto,
            workers_final: 8,
            metrics: OperationMetricsSnapshot::default(),
            elapsed: Duration::from_secs(1),
            failures: Vec::new(),
            failures_truncated: false,
            interrupted: false,
            acl_repair_enabled: false,
            kill_locks_enabled: false,
            remote_locks_enabled: false,
        }
    }

    #[test]
    fn exit_code_success_when_no_failures() {
        assert_eq!(base_summary().exit_code(), ExitCode::Success);
    }

    #[test]
    fn exit_code_completed_with_failures() {
        let mut s = base_summary();
        s.failures.push(DeleteFailure {
            path: PathBuf::from("/tmp/x/f"),
            is_directory: false,
            category: crate::error::FailureCategory::Io,
            message: "boom".to_string(),
            os_error_code: None,
        });
        assert_eq!(s.exit_code(), ExitCode::CompletedWithFailures);
    }

    #[test]
    fn exit_code_interrupted_takes_priority() {
        let mut s = base_summary();
        s.interrupted = true;
        s.failures.push(DeleteFailure {
            path: PathBuf::from("/tmp/x/f"),
            is_directory: false,
            category: crate::error::FailureCategory::Io,
            message: "boom".to_string(),
            os_error_code: None,
        });
        assert_eq!(s.exit_code(), ExitCode::Interrupted);
    }

    #[test]
    fn exit_code_lock_resolution_failed() {
        let mut s = base_summary();
        s.kill_locks_enabled = true;
        s.failures.push(DeleteFailure {
            path: PathBuf::from("/tmp/x/f"),
            is_directory: false,
            category: crate::error::FailureCategory::LocalLockUnresolved,
            message: "still locked".to_string(),
            os_error_code: None,
        });
        assert_eq!(s.exit_code(), ExitCode::LockResolutionFailed);
    }

    #[test]
    fn exit_code_privilege_requirement_not_satisfied() {
        let mut s = base_summary();
        s.acl_repair_enabled = true;
        s.metrics.remediation_attempts = 3;
        s.metrics.remediation_successes = 0;
        s.failures.push(DeleteFailure {
            path: PathBuf::from("/tmp/x/f"),
            is_directory: false,
            category: crate::error::FailureCategory::AccessDenied,
            message: "denied".to_string(),
            os_error_code: None,
        });
        assert_eq!(s.exit_code(), ExitCode::PrivilegeRequirementNotSatisfied);
    }

    #[test]
    fn exit_code_partial_remediation_success_is_generic_failure_not_privilege() {
        let mut s = base_summary();
        s.acl_repair_enabled = true;
        s.metrics.remediation_attempts = 3;
        s.metrics.remediation_successes = 2; // at least one worked
        s.failures.push(DeleteFailure {
            path: PathBuf::from("/tmp/x/f"),
            is_directory: false,
            category: crate::error::FailureCategory::AccessDenied,
            message: "denied".to_string(),
            os_error_code: None,
        });
        assert_eq!(s.exit_code(), ExitCode::CompletedWithFailures);
    }

    #[test]
    fn json_report_round_trips_through_serde() {
        let s = base_summary();
        let json = serde_json::to_string(&s.to_json()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["exitCode"], 0);
    }
}
