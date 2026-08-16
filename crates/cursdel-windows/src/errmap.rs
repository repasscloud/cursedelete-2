//! Maps a Win32 error code (surfaced through `windows::core::Error`'s
//! `HRESULT`) to CurseDelete's platform-independent [`FailureCategory`].

use cursdel_core::error::FailureCategory;
use windows::core::Error as WinError;

/// Extracts the raw Win32 error code embedded in a `windows::core::Error`.
/// The `windows` crate wraps essentially every Win32 API failure as an
/// `HRESULT` built via `HRESULT::from_win32(code)`, which encodes as
/// `0x8007xxxx` (`FACILITY_WIN32 = 7`). Returns `None` for a genuinely
/// COM-native `HRESULT` with no Win32 equivalent (should not occur for any
/// call this crate makes, since none of them are COM APIs, but this must
/// not panic or misclassify if it ever does).
pub fn win32_code(err: &WinError) -> Option<u32> {
    let hr = err.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        Some(hr & 0xFFFF)
    } else {
        None
    }
}

/// True for the two Win32 codes that mean "the object was already gone" --
/// a benign race with a concurrent process or an enumeration artifact, not
/// an operational failure.
pub fn is_not_found(err: &WinError) -> bool {
    matches!(win32_code(err), Some(2) | Some(3)) // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND
}

/// Categorises a Win32 error code into CurseDelete's stable
/// [`FailureCategory`]. `code` is `None` when the originating error was not
/// representable as a Win32 code at all (see [`win32_code`]).
pub fn categorize(code: Option<u32>) -> FailureCategory {
    match code {
        Some(5) => FailureCategory::AccessDenied, // ERROR_ACCESS_DENIED
        Some(32) => FailureCategory::SharingViolation, // ERROR_SHARING_VIOLATION
        Some(33) => FailureCategory::SharingViolation, // ERROR_LOCK_VIOLATION
        Some(2) | Some(3) => FailureCategory::NotFound, // ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND
        Some(145) => FailureCategory::NotEmpty,         // ERROR_DIR_NOT_EMPTY
        Some(206) | Some(111) => FailureCategory::PathTooLong, // ERROR_FILENAME_EXCED_RANGE / ERROR_BUFFER_OVERFLOW
        // ERROR_REPARSE_TAG_INVALID, ERROR_REPARSE_TAG_MISMATCH,
        // ERROR_REPARSE_POINT_ENCOUNTERED: the filesystem itself refused an
        // operation specifically because of the reparse point at the final
        // path component.
        Some(4393) | Some(4394) | Some(4395) => FailureCategory::ReparsePointRefused,
        Some(_) => FailureCategory::Io,
        None => FailureCategory::Other,
    }
}

/// Convenience: categorise a `windows::core::Error` directly.
pub fn categorize_error(err: &WinError) -> FailureCategory {
    categorize(win32_code(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_access_denied() {
        assert_eq!(categorize(Some(5)), FailureCategory::AccessDenied);
    }

    #[test]
    fn maps_sharing_violation() {
        assert_eq!(categorize(Some(32)), FailureCategory::SharingViolation);
    }

    #[test]
    fn maps_lock_violation_as_sharing_violation() {
        assert_eq!(categorize(Some(33)), FailureCategory::SharingViolation);
    }

    #[test]
    fn maps_not_found_codes() {
        assert_eq!(categorize(Some(2)), FailureCategory::NotFound);
        assert_eq!(categorize(Some(3)), FailureCategory::NotFound);
    }

    #[test]
    fn maps_not_empty() {
        assert_eq!(categorize(Some(145)), FailureCategory::NotEmpty);
    }

    #[test]
    fn maps_path_too_long_codes() {
        assert_eq!(categorize(Some(206)), FailureCategory::PathTooLong);
        assert_eq!(categorize(Some(111)), FailureCategory::PathTooLong);
    }

    #[test]
    fn maps_reparse_codes() {
        assert_eq!(categorize(Some(4393)), FailureCategory::ReparsePointRefused);
        assert_eq!(categorize(Some(4394)), FailureCategory::ReparsePointRefused);
        assert_eq!(categorize(Some(4395)), FailureCategory::ReparsePointRefused);
    }

    #[test]
    fn maps_unknown_win32_code_to_io() {
        assert_eq!(categorize(Some(999_999)), FailureCategory::Io);
    }

    #[test]
    fn maps_absent_code_to_other() {
        assert_eq!(categorize(None), FailureCategory::Other);
    }

    #[test]
    fn extracts_win32_code_from_hresult() {
        use windows::core::HRESULT;
        let err = WinError::from(HRESULT::from_win32(5));
        assert_eq!(win32_code(&err), Some(5));
        assert_eq!(categorize_error(&err), FailureCategory::AccessDenied);
    }

    #[test]
    fn is_not_found_true_for_file_and_path_not_found() {
        use windows::core::HRESULT;
        assert!(is_not_found(&WinError::from(HRESULT::from_win32(2))));
        assert!(is_not_found(&WinError::from(HRESULT::from_win32(3))));
        assert!(!is_not_found(&WinError::from(HRESULT::from_win32(5))));
    }
}
