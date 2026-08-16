//! Native Windows deletion engine. Compiles only on `cfg(windows)` targets;
//! see `docs/adr/0001-workspace-architecture.md` for why platform engines
//! are separate crates rather than `cfg`-gated modules inside one crate.
//!
//! Uses the modern `FILE_DISPOSITION_INFO_EX` delete path with a classic
//! `DeleteFileW`/`RemoveDirectoryW` fallback (`ops.rs`), `FindFirstFileExW`
//! with `FIND_FIRST_EX_LARGE_FETCH` for enumeration (`dirlist.rs`), Windows
//! Restart Manager plus direct process termination for `--kill-locks`, and
//! `NetFileEnum`/`NetFileClose` for `--close-remote-locks` (`lock.rs`). See
//! `docs/adr/0007-windows-engine.md` for the full design rationale,
//! including exactly what this crate's TOCTOU/reparse-point safety measures
//! do and do not protect against.

#![cfg(windows)]

mod dirlist;
mod errmap;
mod lock;
mod ops;
mod sys;

use std::path::Path;

use cursdel_core::engine::{
    CancelToken, DeleteAttempt, DeleteOptions, EnumResult, EnumSink, LockResolution,
    PlatformEngine, RemediationOutcome,
};
use cursdel_core::error::DeleteFailure;
use cursdel_core::target::CanonicalTarget;
use cursdel_core::walk::stream_tree;

pub struct WindowsEngine;

impl WindowsEngine {
    /// Best-effort, once-per-process enablement of `SeBackupPrivilege`,
    /// `SeRestorePrivilege`, and `SeTakeOwnershipPrivilege` (see
    /// `sys::enable_privileges_best_effort`). Never fails: an unelevated
    /// process simply will not get these privileges, and plain deletion
    /// (plus ACL-permitted remediation) must keep working regardless.
    pub fn new() -> Self {
        sys::enable_privileges_best_effort();
        Self
    }
}

impl Default for WindowsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformEngine for WindowsEngine {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn enumerate(
        &self,
        root: &CanonicalTarget,
        sink: &EnumSink,
        cancel: &CancelToken,
    ) -> EnumResult {
        stream_tree(root, &dirlist::WindowsDirLister, sink, cancel)
    }

    fn delete_file(&self, path: &Path, opts: DeleteOptions) -> DeleteAttempt {
        ops::delete_file(path, opts)
    }

    fn delete_dir(&self, path: &Path, opts: DeleteOptions) -> DeleteAttempt {
        ops::delete_dir(path, opts)
    }

    fn remediate(
        &self,
        path: &Path,
        is_dir: bool,
        failure: &DeleteFailure,
        opts: DeleteOptions,
    ) -> RemediationOutcome {
        ops::remediate(path, is_dir, failure, opts)
    }

    fn resolve_local_lock(&self, path: &Path, opts: DeleteOptions) -> LockResolution {
        lock::resolve_local_lock(path, opts)
    }

    fn resolve_remote_lock(&self, path: &Path, opts: DeleteOptions) -> LockResolution {
        lock::resolve_remote_lock(path, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cursdel_core::engine::DeleteOutcome;
    use cursdel_core::target::validate_target;

    fn default_opts() -> DeleteOptions {
        DeleteOptions {
            allow_remediation: true,
            kill_locks: false,
            close_remote_locks: false,
        }
    }

    /// End-to-end: run the full cursdel-core pipeline against the real
    /// Windows engine over a real temporary directory tree. Mirrors
    /// `cursdel-macos`'s equivalent test -- see that crate's `lib.rs` for
    /// the pattern this was copied from.
    #[test]
    fn pipeline_deletes_a_nested_tree_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("target");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("b.txt"), b"bb").unwrap();
        std::fs::create_dir(sub.join("empty")).unwrap();

        let engine = WindowsEngine::new();
        let options = cursdel_core::options::OperationOptions {
            target: root.clone(),
            mode: cursdel_core::options::Mode::Normal,
            workers: cursdel_core::options::WorkerPolicy::Fixed(2),
            dry_run: false,
            kill_locks: false,
            close_remote_locks: false,
            filters: cursdel_core::filter::FilterSpec::default(),
            json: false,
            quiet: true,
            verbose: false,
            log_path: None,
        };
        let filters =
            cursdel_core::filter::FilterSet::build(&options.filters).expect("valid filters");

        let summary = cursdel_core::pipeline::run(&engine, &options, &filters, CancelToken::new())
            .expect("target should validate");

        assert_eq!(summary.metrics.files_deleted, 2);
        assert_eq!(summary.metrics.dirs_deleted, 3); // sub, sub/empty, and root itself
        assert_eq!(summary.metrics.failures, 0);
        assert!(!root.exists());
    }

    #[test]
    fn pipeline_root_protection_rejects_filesystem_root() {
        let engine = WindowsEngine::new();
        let options = cursdel_core::options::OperationOptions {
            target: std::path::PathBuf::from(r"C:\"),
            mode: cursdel_core::options::Mode::Normal,
            workers: cursdel_core::options::WorkerPolicy::Fixed(1),
            dry_run: false,
            kill_locks: false,
            close_remote_locks: false,
            filters: cursdel_core::filter::FilterSpec::default(),
            json: false,
            quiet: true,
            verbose: false,
            log_path: None,
        };
        let filters = cursdel_core::filter::FilterSet::build(&options.filters).unwrap();
        let result = cursdel_core::pipeline::run(&engine, &options, &filters, CancelToken::new());
        assert!(result.is_err());
    }

    #[test]
    fn engine_deletes_single_file_directly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("solo.txt");
        std::fs::write(&file, b"x").unwrap();
        let engine = WindowsEngine::new();
        let attempt = engine.delete_file(&file, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
    }

    #[test]
    fn target_validation_accepts_real_subdirectory_of_temp_root() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(validate_target(&child).is_ok());
    }

    #[test]
    fn engine_name_is_windows() {
        assert_eq!(WindowsEngine::new().name(), "windows");
    }

    // TODO(windows-ci): a long-path (>260 char) tree and a real UNC share
    // target both need a live Windows filesystem/session -- the
    // \\?\ / \\?\UNC\ prefixing logic itself is unit-tested in `sys.rs`,
    // but an actual end-to-end pipeline run over a >MAX_PATH tree, and over
    // a genuine \\server\share path, are not exercised by this test module.
}
