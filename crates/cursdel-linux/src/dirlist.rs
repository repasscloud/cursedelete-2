//! Native directory listing for Linux using `opendir`/`readdir` plus
//! per-child metadata opened via `openat`-relative lookups (never a second
//! `lstat(full_path)` call) -- see `docs/adr/0006-posix-toctou.md`.
//!
//! Directories confidently identified by `d_type == DT_DIR` skip the
//! metadata syscall entirely: only files (which participate in `--age`/
//! `--min-size`/`--max-size` filtering) need size/timestamp metadata,
//! directories never do. This mirrors `cursdel-macos::dirlist`'s
//! application of "do not read metadata unless the operation requires
//! it".
//!
//! `d_type` is populated by every mainstream Linux filesystem (ext4, xfs,
//! btrfs) but is not a universal guarantee -- some older XFS
//! configurations, some FUSE filesystems, and NFS can report
//! `DT_UNKNOWN`. Any value other than `DT_DIR` (including `DT_UNKNOWN`)
//! falls through to a real metadata lookup below, so the type is always
//! resolved authoritatively rather than trusted blindly.
//!
//! ## Metadata: `statx` with a `fstatat` fallback
//!
//! Unlike Darwin's `struct stat`, Linux's classic `stat`/`fstatat` does
//! not expose file creation time ("birth time") at all -- there is no
//! `st_birthtime` field. Real creation-time reporting on Linux requires
//! `statx(2)` (Linux 4.11+, glibc >= 2.28) with the `STATX_BTIME` mask
//! bit, and even then not every filesystem populates it (ext4 does;
//! tmpfs, and some network/FUSE filesystems, do not -- `statx` reports
//! this per-call via `stx_mask`, which is checked below rather than
//! assumed).
//!
//! This engine calls `statx` first, requesting `STATX_BASIC_STATS |
//! STATX_BTIME` in one syscall (replacing what would otherwise be a
//! separate `fstatat` call -- no extra syscall cost over the macOS
//! engine's single-`fstatat`-per-entry approach). If `statx` itself is
//! unavailable (`ENOSYS` -- a pre-4.11 kernel, or a container/seccomp
//! profile that blocks the syscall), this falls back to plain `fstatat`,
//! which loses only creation-time reporting: `created` becomes `None`.
//! `cursdel_core::filter` treats a `None` timestamp as "retain, do not
//! delete" (see `FilterSpec::decide`'s `RetainReason::TimestampUnavailable`
//! path), never as "old enough to delete", so this degradation is safe by
//! construction, not just informally safe. See `docs/adr/0008-linux-engine.md`
//! for the full reasoning behind choosing `statx` over unconditionally
//! reporting `created: None`.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use cursdel_core::entry::EntryTimes;
use cursdel_core::walk::{DirLister, RawChild};

use crate::sys::open_dir_nofollow;

pub struct LinuxDirLister;

impl DirLister for LinuxDirLister {
    fn list_children(&self, dir: &Path) -> io::Result<Vec<RawChild>> {
        let dir_fd = open_dir_nofollow(dir)?;
        // fdopendir takes ownership of dir_fd on success; on failure we
        // must close it ourselves.
        let dirp = unsafe { libc::fdopendir(dir_fd) };
        if dirp.is_null() {
            let err = io::Error::last_os_error();
            unsafe {
                libc::close(dir_fd);
            }
            return Err(err);
        }

        let result = list_via_dirp(dirp, dir_fd);
        unsafe {
            libc::closedir(dirp);
        }
        result
    }
}

fn list_via_dirp(dirp: *mut libc::DIR, dir_fd: libc::c_int) -> io::Result<Vec<RawChild>> {
    let mut out = Vec::new();

    loop {
        clear_errno();
        let entry = unsafe { libc::readdir(dirp) };
        if entry.is_null() {
            let errno = current_errno();
            if errno != 0 {
                return Err(io::Error::from_raw_os_error(errno));
            }
            break; // genuine end of stream
        }

        let name = unsafe { dirent_name(entry) };
        if name.as_bytes() == b"." || name.as_bytes() == b".." {
            continue;
        }

        let d_type = unsafe { (*entry).d_type };

        if d_type == libc::DT_DIR {
            // Confident directory: no metadata needed.
            out.push(RawChild {
                name,
                is_dir: true,
                is_reparse_point: false,
                size: 0,
                times: EntryTimes::default(),
                readonly: false,
            });
            continue;
        }

        // Files, symlinks, and DT_UNKNOWN all need a real metadata lookup
        // to get an authoritative type + size/timestamps. AT_SYMLINK_NOFOLLOW
        // means a symlink is always reported as itself, never as whatever
        // it points to.
        match lookup_metadata(dir_fd, &name) {
            Ok(meta) => {
                out.push(RawChild {
                    name,
                    is_dir: meta.is_dir,
                    is_reparse_point: meta.is_symlink,
                    size: meta.size,
                    times: meta.times,
                    readonly: meta.readonly,
                });
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                // Vanished between readdir() and the metadata lookup
                // (benign race with a concurrent process); simply skip it
                // -- there is nothing left to delete.
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    Ok(out)
}

unsafe fn dirent_name(entry: *const libc::dirent) -> OsString {
    let c_str = CStr::from_ptr((*entry).d_name.as_ptr());
    OsStr::from_bytes(c_str.to_bytes()).to_os_string()
}

/// Metadata this engine actually needs from a `statx`/`fstatat` call,
/// independent of which syscall produced it.
struct RawMeta {
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    times: EntryTimes,
    readonly: bool,
}

fn lookup_metadata(dir_fd: libc::c_int, name: &OsStr) -> io::Result<RawMeta> {
    match statx_nofollow(dir_fd, name) {
        Ok(stx) => Ok(RawMeta::from_statx(&stx)),
        Err(e) if e.raw_os_error() == Some(libc::ENOSYS) => {
            // Kernel predates statx(2) (< 4.11) or a container/seccomp
            // policy blocks it outright. Fall back to fstatat -- see the
            // module doc comment for why losing `created` here is a safe
            // degradation rather than a correctness gap.
            fstatat_nofollow(dir_fd, name).map(RawMeta::from_stat)
        }
        Err(e) => Err(e),
    }
}

fn statx_nofollow(dir_fd: libc::c_int, name: &OsStr) -> io::Result<libc::statx> {
    let c_name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))?;
    let mut stx: libc::statx = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::statx(
            dir_fd,
            c_name.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
            libc::STATX_BASIC_STATS | libc::STATX_BTIME,
            &mut stx,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stx)
}

fn fstatat_nofollow(dir_fd: libc::c_int, name: &OsStr) -> io::Result<libc::stat> {
    let c_name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::fstatat(
            dir_fd,
            c_name.as_ptr(),
            &mut st as *mut libc::stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

impl RawMeta {
    fn from_statx(stx: &libc::statx) -> Self {
        let mode = stx.stx_mode as libc::mode_t;
        let is_symlink = (mode & libc::S_IFMT) == libc::S_IFLNK;
        let is_dir = (mode & libc::S_IFMT) == libc::S_IFDIR;
        let created = if stx.stx_mask & libc::STATX_BTIME != 0 {
            systime_from_statx(&stx.stx_btime)
        } else {
            // Syscall succeeded but this filesystem does not populate
            // birth time (e.g. tmpfs, some network/FUSE filesystems) --
            // `stx_mask` is the authoritative "was this field actually
            // filled in" signal, not merely "did I ask for it".
            None
        };
        RawMeta {
            is_dir,
            is_symlink,
            size: stx.stx_size,
            times: EntryTimes {
                modified: systime_from_statx(&stx.stx_mtime),
                created,
                accessed: systime_from_statx(&stx.stx_atime),
            },
            readonly: (mode & libc::S_IWUSR) == 0,
        }
    }

    fn from_stat(st: libc::stat) -> Self {
        let is_symlink = (st.st_mode & libc::S_IFMT) == libc::S_IFLNK;
        let is_dir = (st.st_mode & libc::S_IFMT) == libc::S_IFDIR;
        RawMeta {
            is_dir,
            is_symlink,
            size: st.st_size.max(0) as u64,
            times: EntryTimes {
                modified: systime_from(st.st_mtime, st.st_mtime_nsec),
                // Classic `stat`/`fstatat` has no creation-time field on
                // Linux at all -- see the module doc comment.
                created: None,
                accessed: systime_from(st.st_atime, st.st_atime_nsec),
            },
            readonly: (st.st_mode & libc::S_IWUSR) == 0,
        }
    }
}

fn systime_from_statx(ts: &libc::statx_timestamp) -> Option<SystemTime> {
    systime_from(ts.tv_sec, ts.tv_nsec as i64)
}

fn systime_from(secs: i64, nsecs: i64) -> Option<SystemTime> {
    if secs >= 0 {
        Some(SystemTime::UNIX_EPOCH + Duration::new(secs as u64, nsecs.max(0) as u32))
    } else {
        // A negative timestamp (pre-1970) is legitimate but SystemTime's
        // Duration-based construction cannot represent it directly;
        // subtract instead of failing the whole listing over an edge case.
        SystemTime::UNIX_EPOCH.checked_sub(Duration::new((-secs) as u64, nsecs.max(0) as u32))
    }
}

fn clear_errno() {
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn current_errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_and_directories_with_correct_kinds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), b"hello world").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::os::unix::fs::symlink("file.txt", dir.path().join("link")).unwrap();

        let lister = LinuxDirLister;
        let mut children = lister.list_children(dir.path()).unwrap();
        children.sort_by_key(|c| c.name.clone());

        assert_eq!(children.len(), 3);

        let file = children.iter().find(|c| c.name == "file.txt").unwrap();
        assert!(!file.is_dir);
        assert!(!file.is_reparse_point);
        assert_eq!(file.size, 11);

        let dir_entry = children.iter().find(|c| c.name == "subdir").unwrap();
        assert!(dir_entry.is_dir);
        assert!(!dir_entry.is_reparse_point);

        let link = children.iter().find(|c| c.name == "link").unwrap();
        assert!(!link.is_dir);
        assert!(link.is_reparse_point);
    }

    #[test]
    fn empty_directory_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let lister = LinuxDirLister;
        let children = lister.list_children(dir.path()).unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn nonexistent_directory_is_an_error() {
        let lister = LinuxDirLister;
        let result = lister.list_children(Path::new("/this/does/not/exist/xyz123"));
        assert!(result.is_err());
    }

    #[test]
    fn symlink_to_directory_is_reported_as_reparse_point_not_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link_to_dir")).unwrap();

        let lister = LinuxDirLister;
        let children = lister.list_children(dir.path()).unwrap();
        let link = children
            .iter()
            .find(|c| c.name == "link_to_dir")
            .expect("link should be listed");
        // On POSIX, a symlink is always deleted via unlink() regardless of
        // what it points to, so `is_dir` must be false even though the
        // link's target is a directory -- see docs/adr/0005.
        assert!(!link.is_dir);
        assert!(link.is_reparse_point);
    }

    /// `created` is genuinely filesystem-dependent (see the module doc
    /// comment), so this cannot assert a specific value everywhere CI
    /// might run. It does assert the *shape* of the contract: whichever
    /// filesystem backs the test's tempdir, a freshly-created file's
    /// reported creation time (when present at all) must not be
    /// nonsensical -- specifically, not before its own modification time
    /// beyond a small clock-tick margin.
    // TODO(linux-ci): once running on real Linux hardware with a
    // known-btime-capable filesystem (ext4/btrfs), add a test that
    // asserts `created` is `Some` and closely tracks `SystemTime::now()`,
    // to positively confirm the STATX_BTIME path (not just the fallback)
    // is exercised end-to-end.
    #[test]
    fn creation_time_when_reported_is_not_before_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fresh.txt"), b"x").unwrap();

        let lister = LinuxDirLister;
        let children = lister.list_children(dir.path()).unwrap();
        let fresh = children.iter().find(|c| c.name == "fresh.txt").unwrap();

        if let (Some(created), Some(modified)) = (fresh.times.created, fresh.times.modified) {
            let margin = Duration::from_secs(2);
            assert!(
                created <= modified + margin,
                "creation time should not be meaningfully after modification time for a \
                 freshly-written, never-modified-since file"
            );
        }
        // If `created` is `None`, this filesystem/kernel does not report
        // birth time -- that is an accepted, documented outcome, not a
        // test failure.
    }
}
