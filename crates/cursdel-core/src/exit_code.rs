//! Stable, documented process exit codes.
//!
//! These values are part of CurseDelete's automation contract (see
//! `docs/EXIT_CODES.md`) and must not be renumbered once released. Add new
//! codes rather than repurposing old ones.

/// A stable exit code returned by the `cursdel` process.
///
/// The discriminants are frozen: automation and scripts depend on the
/// numeric values, not just the variant names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Operation completed successfully with zero failures.
    Success = 0,
    /// Invalid arguments, or the target was rejected by safety validation
    /// (root/share-root protection, canonicalisation escape, etc).
    InvalidArgsOrTarget = 1,
    /// The operation ran to completion but one or more objects could not be
    /// deleted (and were not simply filtered/retained on purpose).
    CompletedWithFailures = 2,
    /// A required permission/privilege was not satisfied (e.g. `--force`
    /// requested without the authority needed to remediate).
    PrivilegeRequirementNotSatisfied = 3,
    /// Local lock resolution (`--kill-locks`) was requested and failed to
    /// free one or more locked objects.
    LockResolutionFailed = 4,
    /// Remote SMB lock handling (`--close-remote-locks`) was requested and
    /// failed.
    RemoteLockHandlingFailed = 5,
    /// The operation was interrupted by the user (e.g. Ctrl+C) before it
    /// could finish; partial results are reported.
    Interrupted = 6,
    /// Licence verification failed and the requested capability requires a
    /// valid entitlement.
    LicenseRequired = 7,
    /// Generic CLI usage error (bad flag combination, unknown flag).
    CliUsageError = 64,
    /// Unexpected fatal error not covered by a more specific code.
    UnexpectedFatal = 99,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}

impl std::fmt::Display for ExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}
