use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur while validating a destructive target before any
/// deletion work begins. These map to [`crate::exit_code::ExitCode::InvalidArgsOrTarget`].
#[derive(Debug, Error)]
pub enum TargetError {
    #[error("target path does not exist: {0}")]
    NotFound(PathBuf),

    #[error("failed to canonicalise target path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "refusing to delete filesystem root '{0}': CurseDelete never deletes a drive, \
         volume, or filesystem root. Pass a path at least one level below the root."
    )]
    FilesystemRoot(PathBuf),

    #[error(
        "refusing to delete SMB/UNC share root '{0}': CurseDelete never deletes an entire \
         network share. Pass a path at least one level below the share root."
    )]
    ShareRoot(PathBuf),

    #[error(
        "target '{requested}' resolves to protected root '{resolved}' after canonicalisation \
         (e.g. via '..' components); refusing to proceed"
    )]
    ResolvesToRoot {
        requested: PathBuf,
        resolved: PathBuf,
    },

    #[error("target path is empty")]
    EmptyPath,

    #[error("target '{0}' is neither a file nor a directory (special file, device, etc.)")]
    UnsupportedFileType(PathBuf),
}

/// A single failure encountered while deleting or remediating one filesystem
/// object. These are always collected and reported -- CurseDelete never
/// silently discards a failed deletion.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFailure {
    pub path: PathBuf,
    pub is_directory: bool,
    pub category: FailureCategory,
    pub message: String,
    /// Raw OS error code, if any, for machine consumption.
    pub os_error_code: Option<i32>,
}

/// A coarse, stable categorisation of why a deletion failed. `--json` output
/// depends on these names remaining stable; see `docs/JSON_OUTPUT.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    AccessDenied,
    SharingViolation,
    NotFound,
    NotEmpty,
    PathTooLong,
    ReparsePointRefused,
    OutsideBoundary,
    RemoteLockUnsupported,
    RemoteLockFailed,
    LocalLockUnresolved,
    Io,
    Other,
}

impl std::fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FailureCategory::AccessDenied => "access_denied",
            FailureCategory::SharingViolation => "sharing_violation",
            FailureCategory::NotFound => "not_found",
            FailureCategory::NotEmpty => "not_empty",
            FailureCategory::PathTooLong => "path_too_long",
            FailureCategory::ReparsePointRefused => "reparse_point_refused",
            FailureCategory::OutsideBoundary => "outside_boundary",
            FailureCategory::RemoteLockUnsupported => "remote_lock_unsupported",
            FailureCategory::RemoteLockFailed => "remote_lock_failed",
            FailureCategory::LocalLockUnresolved => "local_lock_unresolved",
            FailureCategory::Io => "io",
            FailureCategory::Other => "other",
        };
        f.write_str(s)
    }
}
