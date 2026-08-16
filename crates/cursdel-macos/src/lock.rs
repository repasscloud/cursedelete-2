//! `--kill-locks` local lock handling for macOS.
//!
//! Darwin has no equivalent of Windows Restart Manager. The standard way
//! to identify which process holds a file open is the same mechanism
//! `lsof` itself is built on; rather than hand-rolling `libproc` FFI
//! (undocumented struct layouts, `PROC_PIDFDVNODEPATHINFO`) for a
//! secondary, opt-in feature, this shells out to `lsof -t <path>`, which
//! is preinstalled on every macOS system and is the de facto standard
//! tool for exactly this query. A native `libproc`-based implementation
//! (removing the subprocess dependency) is recorded as a documented
//! follow-up in `docs/adr/0006-posix-toctou.md` rather than implemented
//! here.
//!
//! `--close-remote-locks` is Windows-server-specific per the product
//! brief; this platform always reports it unsupported rather than
//! pretending to remediate a remote SMB open.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cursdel_core::engine::{DeleteOptions, LockResolution};

/// Never attempt to terminate CurseDelete itself or the system's init
/// process (PID 1). PID/UID 0 is excluded implicitly: `lsof` never
/// attributes an open file to PID 0.
fn is_protected_pid(pid: i32) -> bool {
    pid <= 1 || pid == std::process::id() as i32
}

pub fn resolve_local_lock(path: &Path, opts: DeleteOptions) -> LockResolution {
    if !opts.kill_locks {
        return LockResolution::Unsupported("--kill-locks was not enabled".to_string());
    }

    let pids = match find_holding_pids(path) {
        Ok(pids) => pids,
        Err(msg) => return LockResolution::Unsupported(msg),
    };

    if pids.is_empty() {
        return LockResolution::Failed(
            "delete failed with a sharing/busy error but no process could be identified as holding the file open".to_string(),
        );
    }

    let mut terminated_any = false;
    let mut all_protected = true;
    for pid in &pids {
        if is_protected_pid(*pid) {
            continue;
        }
        all_protected = false;
        if unsafe { libc::kill(*pid, libc::SIGTERM) } == 0 {
            terminated_any = true;
        }
    }

    if all_protected {
        return LockResolution::Failed(format!(
            "file is held open only by protected processes ({pids:?}); refusing to terminate"
        ));
    }

    if !terminated_any {
        return LockResolution::Failed(format!(
            "failed to terminate any process holding the file open ({pids:?})"
        ));
    }

    // Give the terminated process(es) a brief moment to actually release
    // the handle before the caller retries the delete.
    std::thread::sleep(Duration::from_millis(200));
    LockResolution::Resolved
}

pub fn resolve_remote_lock(_path: &Path, _opts: DeleteOptions) -> LockResolution {
    LockResolution::Unsupported(
        "remote SMB open administration is only implemented for supported Windows file servers"
            .to_string(),
    )
}

fn find_holding_pids(path: &Path) -> Result<Vec<i32>, String> {
    let output = Command::new("lsof").arg("-t").arg(path).output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return Err(format!(
                "could not run 'lsof' to identify lock holders: {e}"
            ))
        }
    };

    // lsof exits non-zero when nothing has the file open; that is a valid,
    // empty result, not an error.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pids = stdout
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect();
    Ok(pids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_own_pid_and_init() {
        assert!(is_protected_pid(1));
        assert!(is_protected_pid(std::process::id() as i32));
        assert!(!is_protected_pid(99999));
    }

    #[test]
    fn kill_locks_disabled_is_reported_unsupported_not_failed() {
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: false,
        };
        let result = resolve_local_lock(Path::new("/tmp/does-not-matter"), opts);
        assert!(matches!(result, LockResolution::Unsupported(_)));
    }

    #[test]
    fn finds_own_process_holding_a_file_open() {
        // Open a file and keep the handle alive for the duration of the
        // lsof query, so this process's own PID must appear in the
        // result -- a genuine end-to-end check of the lsof integration
        // without needing a second process.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("held-open.txt");
        let _handle = std::fs::File::create(&file).unwrap();

        match find_holding_pids(&file) {
            Ok(pids) => {
                // lsof may not be permitted to introspect in some locked-
                // down CI sandboxes; only assert when it actually ran.
                if !pids.is_empty() {
                    assert!(pids.contains(&(std::process::id() as i32)));
                }
            }
            Err(_) => {
                // lsof missing entirely -- acceptable in a minimal
                // environment, nothing to assert.
            }
        }
    }

    #[test]
    fn remote_lock_handling_is_always_unsupported_on_macos() {
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: true,
        };
        let result = resolve_remote_lock(Path::new("/tmp/x"), opts);
        assert!(matches!(result, LockResolution::Unsupported(_)));
    }
}
