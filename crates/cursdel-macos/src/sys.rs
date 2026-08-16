//! Small shared POSIX FFI helpers used by both directory listing and
//! delete operations.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub fn path_to_cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// Opens `path` as a directory descriptor, refusing to follow a symlink at
/// the final component (`O_NOFOLLOW`). Used both to list a directory's
/// children and, on the delete path, to re-open a leaf's parent
/// immediately before `unlinkat` so the removal targets a name within a
/// directory descriptor rather than re-resolving a path string -- see
/// `docs/adr/0006-posix-toctou.md` for exactly what this does and does not
/// protect against.
pub fn open_dir_nofollow(path: &Path) -> io::Result<libc::c_int> {
    let c_path = path_to_cstring(path)?;
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

pub struct OwnedFd(pub libc::c_int);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe {
                libc::close(self.0);
            }
        }
    }
}
