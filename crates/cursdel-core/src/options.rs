//! The resolved set of options for one deletion operation, built by
//! `cursdel-cli` from parsed arguments (and, eventually, licence-derived
//! policy) and consumed by `crate::pipeline`.

use std::path::PathBuf;

use crate::engine::DeleteOptions;
use crate::filter::FilterSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Plain deletion: no permission/ownership/ACL remediation is
    /// attempted on failure.
    Normal,
    /// Attribute/ownership/ACL remediation is attempted on failure, where
    /// the executing security context has authority to do so.
    Force,
    /// Force, plus adaptive maximum-throughput operation and local lock
    /// termination. Does *not* imply `--close-remote-locks`, which remains
    /// a separate, explicit opt-in regardless of mode.
    Destroy,
}

impl Mode {
    pub fn allows_remediation(self) -> bool {
        matches!(self, Mode::Force | Mode::Destroy)
    }

    pub fn implies_kill_locks(self) -> bool {
        matches!(self, Mode::Destroy)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Force => "force",
            Mode::Destroy => "destroy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPolicy {
    Auto,
    Fixed(usize),
}

impl std::str::FromStr for WorkerPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(WorkerPolicy::Auto);
        }
        match s.parse::<usize>() {
            Ok(0) => Err("--workers must be 'auto' or a positive integer, not 0".to_string()),
            Ok(n) => Ok(WorkerPolicy::Fixed(n)),
            Err(_) => Err(format!(
                "--workers value '{s}' is not 'auto' or a positive integer"
            )),
        }
    }
}

impl std::fmt::Display for WorkerPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerPolicy::Auto => write!(f, "auto"),
            WorkerPolicy::Fixed(n) => write!(f, "{n}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auto_case_insensitively() {
        assert_eq!("auto".parse::<WorkerPolicy>().unwrap(), WorkerPolicy::Auto);
        assert_eq!("AUTO".parse::<WorkerPolicy>().unwrap(), WorkerPolicy::Auto);
    }

    #[test]
    fn parses_positive_integers() {
        assert_eq!(
            "64".parse::<WorkerPolicy>().unwrap(),
            WorkerPolicy::Fixed(64)
        );
    }

    #[test]
    fn rejects_zero_and_garbage() {
        assert!("0".parse::<WorkerPolicy>().is_err());
        assert!("banana".parse::<WorkerPolicy>().is_err());
    }
}

/// The fully resolved options for one `cursdel` invocation.
#[derive(Debug, Clone)]
pub struct OperationOptions {
    pub target: PathBuf,
    pub mode: Mode,
    pub workers: WorkerPolicy,
    pub dry_run: bool,
    pub kill_locks: bool,
    pub close_remote_locks: bool,
    pub filters: FilterSpec,
    pub json: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub log_path: Option<PathBuf>,
}

impl OperationOptions {
    /// Whether the target's own root should be preserved rather than
    /// deleted once it becomes empty. Retention/filtered operations clean
    /// up *inside* the target; an unfiltered operation deletes the target
    /// itself, matching plain `cursdel <path>` semantics. See
    /// `docs/RETENTION.md` for the full rationale.
    pub fn preserve_root(&self) -> bool {
        !self.filters.is_noop()
    }

    pub fn effective_kill_locks(&self) -> bool {
        self.kill_locks || self.mode.implies_kill_locks()
    }

    pub fn delete_options(&self) -> DeleteOptions {
        DeleteOptions {
            allow_remediation: self.mode.allows_remediation(),
            kill_locks: self.effective_kill_locks(),
            close_remote_locks: self.close_remote_locks,
        }
    }
}
