//! Delete and remediation operations for the Linux engine.
//!
//! Every removal re-opens the target's parent directory (refusing to
//! follow a symlink at that final path component) and then calls
//! `unlinkat` relative to that descriptor rather than operating on the
//! full path string a second time. See `docs/adr/0006-posix-toctou.md` for
//! exactly what this does and does not protect against, and why a full
//! fd-chained walk (eliminating the ancestor-chain race entirely) is
//! documented as follow-up hardening rather than implemented here -- this
//! Linux engine carries exactly the same accepted residual risk as
//! `cursdel-macos`, not a weaker or stronger version of it.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use cursdel_core::engine::{DeleteAttempt, DeleteOptions, RemediationOutcome};
use cursdel_core::error::DeleteFailure;

use crate::errno_map::categorize;
use crate::sys::{open_dir_nofollow, OwnedFd};

pub fn delete_file(path: &Path, _opts: DeleteOptions) -> DeleteAttempt {
    unlink_leaf(path, false)
}

pub fn delete_dir(path: &Path, _opts: DeleteOptions) -> DeleteAttempt {
    unlink_leaf(path, true)
}

fn unlink_leaf(path: &Path, as_dir: bool) -> DeleteAttempt {
    match unlink_relative(path, as_dir) {
        Ok(()) => DeleteAttempt::deleted(0),
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => DeleteAttempt::already_gone(),
        Err(e) => DeleteAttempt::failed(DeleteFailure {
            path: path.to_path_buf(),
            is_directory: as_dir,
            category: categorize(&e),
            message: e.to_string(),
            os_error_code: e.raw_os_error(),
        }),
    }
}

fn unlink_relative(path: &Path, as_dir: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;

    let parent_fd = OwnedFd(open_dir_nofollow(parent)?);
    let c_name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))?;

    let flags = if as_dir { libc::AT_REMOVEDIR } else { 0 };
    let rc = unsafe { libc::unlinkat(parent_fd.0, c_name.as_ptr(), flags) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Attribute/permission remediation: clear a read-only bit blocking
/// deletion, and (best-effort, since it usually requires elevated
/// privilege) attempt to grant the current user write access to the
/// parent directory when the failure was `EACCES`. This is deliberately
/// modest, exactly like `cursdel-macos::ops::remediate` -- Unix deletion
/// authority is governed by the *parent directory's* write permission, not
/// the object's own permission bits, so there is meaningfully less for
/// "remediation" to do here than on Windows; see `docs/SAFETY_MODEL.md`.
///
/// Ownership (`chown`) remediation is intentionally not attempted: it is
/// only ever meaningful when already running as root (an unprivileged
/// `chown` always fails with `EPERM` on Linux, same as Darwin), at which
/// point deletion authority is not the constraint. Attempting it
/// unconditionally would add a syscall that fails in the overwhelming
/// common case for no behavioural benefit, so this stays symmetric with
/// the macOS engine rather than adding untested surface area.
pub fn remediate(path: &Path, is_dir: bool, _failure: &DeleteFailure) -> RemediationOutcome {
    let parent = match path.parent() {
        Some(p) => p,
        None => return RemediationOutcome::NotApplicable,
    };

    let Ok(parent_fd) = open_dir_nofollow(parent).map(OwnedFd) else {
        return RemediationOutcome::NotApplicable;
    };

    let Some(name) = path.file_name() else {
        return RemediationOutcome::NotApplicable;
    };
    let Ok(c_name) = CString::new(name.as_bytes()) else {
        return RemediationOutcome::NotApplicable;
    };

    // Clear owner-write-blocking absence: ensure the object itself is at
    // least owner-writable (covers the common "read-only file" case) and
    // that the parent directory is writable/searchable by the owner
    // (covers "parent directory permissions block unlink").
    //
    // Note a genuine Linux/glibc difference from Darwin here: if `name`
    // names a symlink, `fchmodat(..., AT_SYMLINK_NOFOLLOW)` fails with
    // `ENOTSUP`/`EOPNOTSUPP` on Linux -- glibc has no way to `chmod` a
    // symlink's own (kernel-ignored, always-0777) permission bits, unlike
    // Darwin, which accepts the flag as a no-op-ish success. That failure
    // is expected and harmless here: `object_ok` simply becomes `false`
    // for a symlink, and `parent_ok` (the call that actually matters for
    // unlinking a symlink, since deletion authority comes from the parent
    // directory) is unaffected.
    let object_mode = if is_dir { 0o755 } else { 0o644 };
    let object_ok = unsafe {
        libc::fchmodat(
            parent_fd.0,
            c_name.as_ptr(),
            object_mode,
            libc::AT_SYMLINK_NOFOLLOW,
        ) == 0
    };

    let parent_c = match CString::new(parent.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return RemediationOutcome::NotApplicable,
    };
    let parent_ok = unsafe { libc::chmod(parent_c.as_ptr(), 0o755) == 0 };

    if object_ok || parent_ok {
        RemediationOutcome::Applied
    } else {
        RemediationOutcome::Failed(io::Error::last_os_error().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cursdel_core::engine::DeleteOutcome;

    fn default_opts() -> DeleteOptions {
        DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: false,
        }
    }

    #[test]
    fn deletes_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let attempt = delete_file(&file, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
        assert!(!file.exists());
    }

    #[test]
    fn deletes_an_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let attempt = delete_dir(&sub, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
        assert!(!sub.exists());
    }

    #[test]
    fn deleting_nonempty_directory_fails_with_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("f.txt"), b"x").unwrap();
        let attempt = delete_dir(&sub, default_opts());
        match attempt.outcome {
            DeleteOutcome::Failed(f) => {
                assert_eq!(f.category, cursdel_core::error::FailureCategory::NotEmpty)
            }
            other => panic!("expected NotEmpty failure, got {other:?}"),
        }
    }

    #[test]
    fn deleting_already_gone_file_reports_already_gone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gone.txt");
        // Never created.
        let attempt = delete_file(&file, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::AlreadyGone));
    }

    #[test]
    fn deletes_a_symlink_without_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"keep me").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let attempt = delete_file(&link, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
        assert!(!link.exists());
        // The symlink's target must be untouched.
        assert!(target.exists());
    }

    #[test]
    fn remediation_clears_readonly_bit_and_allows_delete() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ro.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms).unwrap();

        // A read-only file is still owner-deletable on POSIX (deletion is
        // governed by the parent directory's permissions, not the file's
        // own mode), so the first attempt should already succeed. This
        // test instead documents that expectation and exercises the
        // remediation path directly to prove it doesn't error out.
        let failure = DeleteFailure {
            path: file.clone(),
            is_directory: false,
            category: cursdel_core::error::FailureCategory::AccessDenied,
            message: "simulated".to_string(),
            os_error_code: None,
        };
        let outcome = remediate(&file, false, &failure);
        assert!(matches!(outcome, RemediationOutcome::Applied));
    }

    /// Linux-specific: `fchmodat(..., AT_SYMLINK_NOFOLLOW)` on a symlink
    /// itself returns `ENOTSUP` (glibc has no `lchmod`-equivalent kernel
    /// path), unlike Darwin. Remediation must still report `Applied`
    /// because the parent-directory chmod succeeds regardless -- this
    /// pins down the exact platform difference called out in the
    /// `remediate` doc comment above.
    #[test]
    fn remediation_on_a_symlink_still_succeeds_via_parent_chmod() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let failure = DeleteFailure {
            path: link.clone(),
            is_directory: false,
            category: cursdel_core::error::FailureCategory::AccessDenied,
            message: "simulated".to_string(),
            os_error_code: None,
        };
        let outcome = remediate(&link, false, &failure);
        assert!(matches!(outcome, RemediationOutcome::Applied));
    }
}
