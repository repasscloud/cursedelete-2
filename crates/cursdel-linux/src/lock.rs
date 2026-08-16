//! `--kill-locks` local lock handling for Linux.
//!
//! Linux has no equivalent of Windows Restart Manager, but unlike Darwin
//! (`cursdel-macos::lock` shells out to `lsof -t <path>`, see that module's
//! doc comment for why), Linux exposes lock-holder identification
//! *natively*: `/proc/[pid]/fd/*` are symlinks to every file descriptor a
//! process currently has open, readable directly with `readlink()`. This
//! is the same technique `lsof`/`fuser` use internally on Linux under the
//! hood -- they too just walk `/proc` -- but doing it here in-process
//! avoids a subprocess spawn, a dependency on an external binary being
//! installed and on `$PATH`, and the argument-quoting surface that comes
//! with `Command::new("lsof").arg(path)`. See `docs/adr/0008-linux-engine.md`
//! for the full comparison against the macOS approach.
//!
//! `--close-remote-locks` is Windows-server-specific per the product
//! brief; this platform always reports it unsupported rather than
//! pretending to remediate a remote SMB open.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use cursdel_core::engine::{DeleteOptions, LockResolution};

/// Never attempt to terminate CurseDelete itself, the system's init
/// process (PID 1), or the kernel-thread reaper (PID 2, `kthreadd` --
/// every kernel thread's parent). PID 0 is not a real process on Linux and
/// never appears in `/proc`, so no explicit check is needed for it.
///
/// This is deliberately simpler than exhaustively walking the full
/// process ancestry to find every kernel thread: `is_kernel_thread` below
/// (used only during *discovery*, to avoid wasting time scanning
/// `/proc/[pid]/fd` for processes that structurally cannot hold a
/// userspace file open) already filters out kernel threads by checking
/// for a direct parent of PID 2, which covers the overwhelming majority.
/// This function is the last line of defense at *termination* time and
/// intentionally stays a cheap, unconditional check on well-known PIDs
/// plus our own, rather than re-deriving kernel-thread-ness here too.
fn is_protected_pid(pid: i32) -> bool {
    pid <= 2 || pid == std::process::id() as i32
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

/// Identify every PID with an open file descriptor referring to `path`, by
/// walking `/proc/[pid]/fd/*` and comparing each descriptor's resolved
/// target against `path`'s canonical form.
///
/// Errors reading an individual process's `/proc/[pid]/fd` directory
/// (typically `EACCES` for a process this user does not own and is not
/// privileged to introspect, or `ENOENT` if the process exited mid-scan)
/// are expected and are skipped rather than failing the whole scan --
/// exactly as `README-CurseDelete2.md`'s "use root privileges only where
/// required" rule implies: an unprivileged CurseDelete simply cannot see
/// every process's file descriptors, and that is not a bug. Only a
/// failure to read `/proc` itself is a hard error (returned as
/// `Unsupported`, since it means this mechanism cannot function at all on
/// this host -- e.g. `/proc` unmounted, an extremely locked-down
/// container).
fn find_holding_pids(path: &Path) -> Result<Vec<i32>, String> {
    let canonical_target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let proc_dir = fs::read_dir("/proc")
        .map_err(|e| format!("could not read /proc to identify lock holders: {e}"))?;

    let mut pids = Vec::new();

    for entry in proc_dir.flatten() {
        let Some(pid) = parse_pid(&entry.file_name()) else {
            continue; // not a PID directory (e.g. "self", "net", "sys")
        };

        if is_kernel_thread(pid) {
            // Kernel threads have no meaningful userspace fd table; skip
            // the (pointless) fd scan for them. This is a discovery-time
            // optimisation and *not* the sole protection against
            // terminating one -- `is_protected_pid` above is the
            // authoritative check applied before any `kill()` call.
            continue;
        }

        let fd_dir = format!("/proc/{pid}/fd");
        let fds = match fs::read_dir(&fd_dir) {
            Ok(fds) => fds,
            Err(_) => continue, // not ours to introspect, or it already exited
        };

        for fd_entry in fds.flatten() {
            let Ok(target) = fs::read_link(fd_entry.path()) else {
                continue;
            };
            if fd_target_matches(&target, &canonical_target) {
                pids.push(pid);
                break;
            }
        }
    }

    Ok(pids)
}

fn parse_pid(name: &OsStr) -> Option<i32> {
    let s = name.to_str()?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i32>().ok()
}

/// `/proc/[pid]/fd/N` targets for a still-linked file are the resolved
/// absolute path, exactly as `std::fs::canonicalize` would report it. If
/// the file was unlinked while still open (a legitimate, common state --
/// e.g. a log file a process still has open after rotation deleted it),
/// the kernel appends a literal `" (deleted)"` suffix to the symlink
/// target; strip it before comparing so an already-unlinked-but-still-open
/// file is still correctly matched to its holder.
fn fd_target_matches(target: &Path, canonical_target: &Path) -> bool {
    let target_bytes = target.as_os_str().as_bytes();
    let stripped = target_bytes
        .strip_suffix(b" (deleted)")
        .unwrap_or(target_bytes);
    Path::new(std::ffi::OsStr::from_bytes(stripped)) == canonical_target
}

/// Best-effort kernel-thread detection: a process whose parent is PID 2
/// (`kthreadd`) is a kernel thread and must never be a `--kill-locks`
/// target or a productive fd-scan target. Any failure to read/parse
/// `/proc/[pid]/stat` (process exited between the `/proc` listing and this
/// read, permission denied, unexpected format on some non-mainline
/// kernel) is treated as "not confidently a kernel thread" rather than
/// blocking the scan -- a false negative here only means one process gets
/// an unnecessary (and harmless, since kernel threads have no fd table to
/// match against) fd scan, whereas a false positive would mean skipping a
/// real userspace process. This is a deliberate simplification: full
/// kernel-thread detection via `/proc/[pid]/status`'s `Kthread:` field is
/// not present on all kernel versions, so this uses the portable
/// parent-PID check instead. See `docs/adr/0008-linux-engine.md`.
fn is_kernel_thread(pid: i32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Format: "pid (comm) state ppid ...". `comm` may itself contain
    // spaces or parentheses, so find the *last* ')' to bound it correctly
    // regardless of what the executable named itself.
    let Some(close_paren) = stat.rfind(')') else {
        return false;
    };
    let rest = stat[close_paren + 1..].trim_start();
    let mut fields = rest.split_whitespace();
    let _state = fields.next();
    let Some(ppid_str) = fields.next() else {
        return false;
    };
    ppid_str.parse::<i32>() == Ok(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_own_pid_init_and_kthreadd() {
        assert!(is_protected_pid(1));
        assert!(is_protected_pid(2));
        assert!(is_protected_pid(std::process::id() as i32));
        assert!(!is_protected_pid(999_999));
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

    /// Mirrors `cursdel-macos::lock::tests::finds_own_process_holding_a_file_open`,
    /// adapted to read `/proc` directly instead of shelling out to `lsof`.
    #[test]
    fn finds_own_process_holding_a_file_open() {
        // Open a file and keep the handle alive for the duration of the
        // /proc scan, so this process's own PID must appear in the
        // result -- a genuine end-to-end check of the /proc/[pid]/fd
        // walk without needing a second process.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("held-open.txt");
        let _handle = std::fs::File::create(&file).unwrap();

        let pids = find_holding_pids(&file).expect("/proc should always be readable in CI");
        assert!(
            pids.contains(&(std::process::id() as i32)),
            "expected own pid {} among holders {pids:?}",
            std::process::id()
        );
    }

    #[test]
    fn does_not_find_holders_for_a_file_nothing_has_open() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("never-opened.txt");
        std::fs::write(&file, b"x").unwrap();

        let pids = find_holding_pids(&file).expect("/proc should always be readable in CI");
        assert!(pids.is_empty());
    }

    #[test]
    fn matches_deleted_file_marker_suffix() {
        let real = Path::new("/tmp/some/real/path.txt");
        let with_marker = Path::new("/tmp/some/real/path.txt (deleted)");
        assert!(fd_target_matches(with_marker, real));
        assert!(!fd_target_matches(with_marker, Path::new("/tmp/other.txt")));
    }

    #[test]
    fn parses_only_numeric_proc_entries_as_pids() {
        assert_eq!(parse_pid(OsStr::new("1234")), Some(1234));
        assert_eq!(parse_pid(OsStr::new("self")), None);
        assert_eq!(parse_pid(OsStr::new("net")), None);
        assert_eq!(parse_pid(OsStr::new("")), None);
    }

    #[test]
    fn own_process_is_not_a_kernel_thread() {
        assert!(!is_kernel_thread(std::process::id() as i32));
    }

    #[test]
    fn remote_lock_handling_is_always_unsupported_on_linux() {
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: true,
        };
        let result = resolve_remote_lock(Path::new("/tmp/x"), opts);
        assert!(matches!(result, LockResolution::Unsupported(_)));
    }
}
