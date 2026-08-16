//! Native directory listing for Windows using `FindFirstFileExW` with
//! `FindExInfoBasic` (skips the short/8.3 alternate-name lookup, which
//! `FindExInfoStandard` performs unconditionally) and
//! `FIND_FIRST_EX_LARGE_FETCH` (batches multiple directory entries per
//! kernel round-trip instead of one `FindNextFileW` call per entry) --
//! `README-CurseDelete2.md` section 5 names both as candidate performance
//! primitives, and `_old/sfvdd` already validated this combination works.
//!
//! Every attribute reported here (`is_dir`, `is_reparse_point`, `size`,
//! `times`, `readonly`) comes directly from the single `WIN32_FIND_DATAW`
//! the listing call already returned -- no follow-up `GetFileAttributesW`/
//! `CreateFileW` per entry, matching the product's "do not read metadata
//! unless required" principle (a plain, non-force-mode delete never pays
//! for more than one syscall's worth of metadata per file).

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_NO_MORE_FILES;
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
    FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
};

use cursdel_core::entry::EntryTimes;
use cursdel_core::walk::{DirLister, RawChild};

use crate::sys::{filetime_to_systemtime, to_wide_verbatim};

pub struct WindowsDirLister;

impl DirLister for WindowsDirLister {
    fn list_children(&self, dir: &Path) -> io::Result<Vec<RawChild>> {
        // FindFirstFileExW takes a search pattern, not a bare directory --
        // append `\*` to match every immediate child.
        let mut pattern = dir.to_path_buf().into_os_string();
        pattern.push("\\*");
        let wide_pattern = to_wide_verbatim(Path::new(&pattern));

        let mut data = WIN32_FIND_DATAW::default();
        let handle = unsafe {
            FindFirstFileExW(
                PCWSTR(wide_pattern.as_ptr()),
                FindExInfoBasic,
                &mut data as *mut _ as *mut c_void,
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        let handle = match handle {
            Ok(h) => h,
            Err(e) => return Err(win_error_to_io(&e)),
        };

        let mut out = Vec::new();
        loop {
            let name = wide_to_os_string(&data.cFileName);
            if name != "." && name != ".." {
                out.push(raw_child_from_find_data(&data, name));
            }

            match unsafe { FindNextFileW(handle, &mut data) } {
                Ok(()) => continue,
                Err(e) => {
                    let _ = unsafe { FindClose(handle) };
                    return if is_no_more_files(&e) {
                        Ok(out)
                    } else {
                        // A genuine mid-listing error (e.g. a concurrent
                        // ACL change denies further access). Report the
                        // whole directory as failed rather than returning
                        // a silently truncated partial listing -- a
                        // truncated success would mean some children are
                        // simply never discovered, never deleted, and
                        // never reported as failed either.
                        Err(win_error_to_io(&e))
                    };
                }
            }
        }
    }
}

fn raw_child_from_find_data(data: &WIN32_FIND_DATAW, name: std::ffi::OsString) -> RawChild {
    let attrs = data.dwFileAttributes;
    let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
    let is_reparse_point = (attrs & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;
    let readonly = (attrs & FILE_ATTRIBUTE_READONLY.0) != 0;
    let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;

    RawChild {
        name,
        is_dir,
        is_reparse_point,
        size,
        times: EntryTimes {
            modified: filetime_to_systemtime(data.ftLastWriteTime),
            created: filetime_to_systemtime(data.ftCreationTime),
            accessed: filetime_to_systemtime(data.ftLastAccessTime),
        },
        readonly,
    }
}

fn wide_to_os_string(buf: &[u16]) -> std::ffi::OsString {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    std::ffi::OsString::from_wide(&buf[..len])
}

fn is_no_more_files(err: &windows::core::Error) -> bool {
    crate::errmap::win32_code(err) == Some(ERROR_NO_MORE_FILES.0)
}

fn win_error_to_io(err: &windows::core::Error) -> io::Error {
    match crate::errmap::win32_code(err) {
        Some(code) => io::Error::from_raw_os_error(code as i32),
        None => io::Error::other(err.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_and_directories_with_correct_kinds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), b"hello world").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();

        let lister = WindowsDirLister;
        let mut children = lister.list_children(dir.path()).unwrap();
        children.sort_by_key(|c| c.name.clone());

        assert_eq!(children.len(), 2);

        let file = children.iter().find(|c| c.name == "file.txt").unwrap();
        assert!(!file.is_dir);
        assert!(!file.is_reparse_point);
        assert_eq!(file.size, 11);

        let dir_entry = children.iter().find(|c| c.name == "subdir").unwrap();
        assert!(dir_entry.is_dir);
        assert!(!dir_entry.is_reparse_point);
    }

    #[test]
    fn empty_directory_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let lister = WindowsDirLister;
        let children = lister.list_children(dir.path()).unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn nonexistent_directory_is_an_error() {
        let lister = WindowsDirLister;
        let result = lister.list_children(Path::new(r"C:\this\does\not\exist\xyz123"));
        assert!(result.is_err());
    }

    #[test]
    fn hidden_and_system_files_are_listed_not_skipped() {
        // Product requirement: CurseDelete deletes hidden/system objects
        // like any other -- --force/attribute remediation exists to clear
        // the attributes that would otherwise block deletion, not to make
        // the enumerator skip them.
        use windows::Win32::Storage::FileSystem::{
            SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
        };

        let dir = tempfile::tempdir().unwrap();
        let hidden = dir.path().join("hidden.txt");
        std::fs::write(&hidden, b"x").unwrap();
        let wide = to_wide_verbatim(&hidden);
        unsafe {
            SetFileAttributesW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_HIDDEN.0),
            )
            .unwrap();
        }

        let lister = WindowsDirLister;
        let children = lister.list_children(dir.path()).unwrap();
        assert!(children.iter().any(|c| c.name == "hidden.txt"));
    }

    // TODO(windows-ci): junction/directory-symlink `is_reparse_point`
    // reporting (requires `mklink /J` or `CreateSymbolicLinkW`, which in
    // turn generally requires either SeCreateSymbolicLinkPrivilege/Developer
    // Mode or an elevated session) can only be exercised on a live Windows
    // filesystem -- see `ops.rs` tests for the corresponding delete-side
    // coverage gap.
}
