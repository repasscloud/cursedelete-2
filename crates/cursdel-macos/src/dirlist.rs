//! Native directory listing for macOS using `opendir`/`readdir` plus
//! `fstatat(..., AT_SYMLINK_NOFOLLOW)` for metadata, opened via `openat`
//! relative to the already-open parent directory descriptor rather than by
//! re-resolving a path string -- see `docs/adr/0006-posix-toctou.md`.
//!
//! Directories confidently identified by `d_type == DT_DIR` skip the
//! `fstatat` call entirely: only files (which participate in `--age`/
//! `--min-size`/`--max-size` filtering) need size/timestamp metadata,
//! directories never do. This is a direct application of "do not read
//! metadata unless the operation requires it."

use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use cursdel_core::entry::EntryTimes;
use cursdel_core::walk::{DirLister, RawChild};

use crate::sys::open_dir_nofollow;

pub struct MacDirLister;

impl DirLister for MacDirLister {
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

        // Files, symlinks, and DT_UNKNOWN all need a real stat to get
        // authoritative type + size/timestamps. AT_SYMLINK_NOFOLLOW means a
        // symlink is always reported as itself, never as whatever it
        // points to.
        match fstatat_nofollow(dir_fd, &name) {
            Ok(st) => {
                let is_symlink = (st.st_mode & libc::S_IFMT) == libc::S_IFLNK;
                let is_dir = (st.st_mode & libc::S_IFMT) == libc::S_IFDIR;
                out.push(RawChild {
                    name,
                    is_dir,
                    is_reparse_point: is_symlink,
                    size: st.st_size.max(0) as u64,
                    times: entry_times_from_stat(&st),
                    readonly: (st.st_mode & libc::S_IWUSR) == 0,
                });
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                // Vanished between readdir() and fstatat() (benign race
                // with a concurrent process); simply skip it -- there is
                // nothing left to delete.
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

fn entry_times_from_stat(st: &libc::stat) -> EntryTimes {
    EntryTimes {
        modified: systime_from(st.st_mtime, st.st_mtime_nsec),
        created: systime_from(st.st_birthtime, st.st_birthtime_nsec),
        accessed: systime_from(st.st_atime, st.st_atime_nsec),
    }
}

fn systime_from(secs: libc::time_t, nsecs: i64) -> Option<SystemTime> {
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
        *libc::__error() = 0;
    }
}

fn current_errno() -> i32 {
    unsafe { *libc::__error() }
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

        let lister = MacDirLister;
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
        let lister = MacDirLister;
        let children = lister.list_children(dir.path()).unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn nonexistent_directory_is_an_error() {
        let lister = MacDirLister;
        let result = lister.list_children(Path::new("/this/does/not/exist/xyz123"));
        assert!(result.is_err());
    }

    #[test]
    fn symlink_to_directory_is_reported_as_reparse_point_not_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link_to_dir")).unwrap();

        let lister = MacDirLister;
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
}
