//! Maps a POSIX `errno` (surfaced through `std::io::Error`) to CurseDelete's
//! platform-independent [`FailureCategory`].

use cursdel_core::error::FailureCategory;

pub fn categorize(err: &std::io::Error) -> FailureCategory {
    match err.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => FailureCategory::AccessDenied,
        Some(libc::EBUSY) | Some(libc::ETXTBSY) => FailureCategory::SharingViolation,
        Some(libc::ENOENT) => FailureCategory::NotFound,
        Some(libc::ENOTEMPTY) => FailureCategory::NotEmpty,
        Some(libc::ENAMETOOLONG) => FailureCategory::PathTooLong,
        Some(libc::ELOOP) => FailureCategory::ReparsePointRefused,
        _ => FailureCategory::Io,
    }
}
