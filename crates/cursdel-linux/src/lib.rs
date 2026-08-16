//! Native Linux deletion engine. Compiles only on `cfg(target_os =
//! "linux")` targets.
//!
//! Uses Linux/POSIX-native primitives throughout (`opendir`/`readdir`,
//! `openat`/`unlinkat`, `statx`/`fstatat`) -- structurally close to
//! `cursdel-macos` (same TOCTOU mitigation pattern, same `d_type`-based
//! directory-listing optimisation), adjusted where Linux and Darwin
//! genuinely differ: no `st_birthtime` on classic `stat` (this engine uses
//! `statx` instead, see `dirlist.rs`), and `--kill-locks` lock-holder
//! identification via `/proc/[pid]/fd/*` natively instead of shelling out
//! to `lsof` (see `lock.rs`). `--close-remote-locks` is always reported
//! unsupported, per the product brief's requirement that this remain a
//! Windows-server-specific capability. See `docs/adr/0008-linux-engine.md`
//! for the full reasoning behind each of these choices.

#![cfg(target_os = "linux")]

mod dirlist;
mod errno_map;
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

pub struct LinuxEngine;

impl PlatformEngine for LinuxEngine {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn enumerate(
        &self,
        root: &CanonicalTarget,
        sink: &EnumSink,
        cancel: &CancelToken,
    ) -> EnumResult {
        stream_tree(root, &dirlist::LinuxDirLister, sink, cancel)
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
        _opts: DeleteOptions,
    ) -> RemediationOutcome {
        ops::remediate(path, is_dir, failure)
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
    /// Linux engine over a real temporary directory tree. This is the
    /// closest thing to an integration test this crate has, and it
    /// exercises enumeration, filtering (a no-op filter set here), the
    /// worker pool, and directory finalisation together. Mirrors
    /// `cursdel-macos::tests::pipeline_deletes_a_nested_tree_end_to_end`.
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

        let engine = LinuxEngine;
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
        let engine = LinuxEngine;
        let options = cursdel_core::options::OperationOptions {
            target: std::path::PathBuf::from("/"),
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
        let engine = LinuxEngine;
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
}
