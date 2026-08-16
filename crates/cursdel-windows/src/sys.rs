//! Small shared Win32 helpers used across directory listing, delete
//! operations, and lock resolution: long-path/UNC prefixing, `FILETIME`
//! conversion, and best-effort token privilege enablement.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, FILETIME, HANDLE, LUID,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_BACKUP_NAME,
    SE_PRIVILEGE_ENABLED, SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Converts `path` to a long-path-safe, verbatim (`\\?\` / `\\?\UNC\`)
/// absolute form suitable for every Win32 filesystem call this crate makes.
/// See `docs/adr/0007-windows-engine.md` for why this is applied
/// unconditionally rather than only for paths that exceed `MAX_PATH`: the
/// verbatim prefix disables Win32's own path normalisation entirely, which
/// is exactly what a deletion tool wants (no silent `.`/`..`/separator
/// reinterpretation between "this is the path the walker discovered" and
/// "this is the path the delete syscall receives").
///
/// A relative input (should not normally occur -- see module docs on
/// `CanonicalTarget::requested`, which single-object deletes pass through
/// unmodified) is first resolved against the current directory, lexically
/// (no symlink resolution -- this crate must never follow a symlink while
/// deciding what to delete).
pub fn to_verbatim(path: &Path) -> PathBuf {
    let absolute = make_absolute(path);
    // A verbatim path does not get Win32's usual `/` -> `\` normalisation,
    // so it must be done explicitly here (see `handles_mixed_separators`
    // test below and `cursdel_core::target`'s analogous lexical layer).
    let normalized = absolute.to_string_lossy().replace('/', "\\");

    if normalized.starts_with(r"\\?\") {
        // Already verbatim (this is in fact the common case: on Windows,
        // `std::fs::canonicalize` -- used to build `CanonicalTarget::canonical`,
        // which the tree walker operates on -- already returns a `\\?\`-
        // prefixed path).
        PathBuf::from(normalized)
    } else if let Some(rest) = normalized.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{rest}"))
    } else {
        PathBuf::from(format!(r"\\?\{normalized}"))
    }
}

fn make_absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// [`to_verbatim`], encoded as a NUL-terminated UTF-16 buffer ready for
/// `PCWSTR::from_raw`/`PCWSTR(ptr)`.
pub fn to_wide_verbatim(path: &Path) -> Vec<u16> {
    let verbatim = to_verbatim(path);
    let mut wide: Vec<u16> = verbatim.as_os_str().encode_wide().collect();
    wide.push(0);
    wide
}

/// Converts a Win32 `FILETIME` (100ns intervals since 1601-01-01 UTC) to
/// [`SystemTime`]. Returns `None` for an all-zero `FILETIME`, which
/// `FindFirstFileExW`/`GetFileInformationByHandle` use to mean "not
/// available on this filesystem" rather than "the Windows epoch itself" --
/// matching `EntryTimes`'s documented `None` convention.
pub fn filetime_to_systemtime(ft: FILETIME) -> Option<SystemTime> {
    let ticks: u64 = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    if ticks == 0 {
        return None;
    }

    // Number of 100ns intervals between the Windows epoch (1601-01-01) and
    // the Unix epoch (1970-01-01): 11,644,473,600 seconds * 10,000,000.
    const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;

    if ticks >= EPOCH_DIFF_100NS {
        let unix_100ns = ticks - EPOCH_DIFF_100NS;
        let secs = unix_100ns / 10_000_000;
        let nanos = ((unix_100ns % 10_000_000) * 100) as u32;
        Some(SystemTime::UNIX_EPOCH + Duration::new(secs, nanos))
    } else {
        // Pre-1970 timestamp: legitimate (e.g. a file with a forged or
        // very old creation time), but must be built via subtraction since
        // Duration cannot represent a negative offset.
        let diff_100ns = EPOCH_DIFF_100NS - ticks;
        let secs = diff_100ns / 10_000_000;
        let nanos = ((diff_100ns % 10_000_000) * 100) as u32;
        SystemTime::UNIX_EPOCH.checked_sub(Duration::new(secs, nanos))
    }
}

/// Best-effort, once-per-process enablement of the privileges the
/// remediation path (`ops::remediate`) needs to take ownership of, and
/// grant ACLs on, objects the current token does not already have
/// discretionary access to: `SeBackupPrivilege`, `SeRestorePrivilege`,
/// `SeTakeOwnershipPrivilege`. Called once from [`crate::WindowsEngine::new`].
///
/// A non-elevated token simply will not hold these privileges (or will
/// hold them disabled and un-enablable), and `AdjustTokenPrivileges`
/// reports that with `ERROR_NOT_ALL_ASSIGNED` rather than a hard failure --
/// this function logs that at `debug` level and moves on. Plain,
/// unprivileged deletion (and remediation the ACL already permits without
/// elevation) must keep working either way; see
/// `docs/adr/0007-windows-engine.md`.
pub fn enable_privileges_best_effort() {
    for name in [SE_BACKUP_NAME, SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME] {
        if let Err(message) = enable_privilege(name) {
            let readable = unsafe { name.to_string() }.unwrap_or_else(|_| "<privilege>".into());
            tracing::debug!(
                privilege = %readable,
                reason = %message,
                "could not enable Windows privilege at startup (process is likely not \
                 elevated); unprivileged delete and ACL-permitted remediation still work"
            );
        }
    }
}

fn enable_privilege(name: PCWSTR) -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .map_err(|e| e.to_string())?;

        let outcome = (|| -> Result<(), String> {
            let mut luid = LUID::default();
            LookupPrivilegeValueW(None, name, &mut luid).map_err(|e| e.to_string())?;

            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None)
                .map_err(|e| e.to_string())?;

            // AdjustTokenPrivileges reports a partial failure (privilege
            // present in the token but not actually enabled -- the normal
            // case for an unelevated admin token) via GetLastError, not
            // via its own return value.
            if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                return Err(
                    "privilege present in token but could not be enabled (not elevated?)"
                        .to_string(),
                );
            }
            Ok(())
        })();

        let _ = CloseHandle(token);
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_drive_path() {
        let out = to_verbatim(Path::new(r"C:\Temp\Old"));
        assert_eq!(out, PathBuf::from(r"\\?\C:\Temp\Old"));
    }

    #[test]
    fn leaves_already_verbatim_drive_path_unchanged() {
        let out = to_verbatim(Path::new(r"\\?\C:\Temp\Old"));
        assert_eq!(out, PathBuf::from(r"\\?\C:\Temp\Old"));
    }

    #[test]
    fn prefixes_unc_path() {
        let out = to_verbatim(Path::new(r"\\server\share\folder"));
        assert_eq!(out, PathBuf::from(r"\\?\UNC\server\share\folder"));
    }

    #[test]
    fn leaves_already_verbatim_unc_path_unchanged() {
        let out = to_verbatim(Path::new(r"\\?\UNC\server\share\folder"));
        assert_eq!(out, PathBuf::from(r"\\?\UNC\server\share\folder"));
    }

    #[test]
    fn normalizes_forward_slashes_before_prefixing() {
        // Verbatim paths are not normalised by Win32, so mixed separators
        // must be fixed up before the prefix is added, or the API call
        // would see a literal (invalid) `/` path component.
        let out = to_verbatim(Path::new("C:/Temp/Old"));
        assert_eq!(out, PathBuf::from(r"\\?\C:\Temp\Old"));
    }

    #[test]
    fn wide_verbatim_is_nul_terminated() {
        let wide = to_wide_verbatim(Path::new(r"C:\Temp\Old"));
        assert_eq!(*wide.last().unwrap(), 0u16);
        // No interior NULs before the terminator.
        assert!(wide[..wide.len() - 1].iter().all(|&c| c != 0));
    }

    #[test]
    fn filetime_zero_is_unavailable_not_epoch() {
        assert!(filetime_to_systemtime(FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        })
        .is_none());
    }

    #[test]
    fn filetime_at_unix_epoch_round_trips() {
        // 116,444,736,000,000,000 100ns ticks = exactly 1970-01-01T00:00:00Z.
        const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
        let ft = FILETIME {
            dwLowDateTime: (EPOCH_DIFF_100NS & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (EPOCH_DIFF_100NS >> 32) as u32,
        };
        assert_eq!(filetime_to_systemtime(ft), Some(SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn filetime_after_epoch_round_trips_to_expected_unix_seconds() {
        // 2021-01-01T00:00:00Z: unix seconds 1_609_459_200.
        const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
        let unix_secs: u64 = 1_609_459_200;
        let ticks = EPOCH_DIFF_100NS + unix_secs * 10_000_000;
        let ft = FILETIME {
            dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(unix_secs);
        assert_eq!(filetime_to_systemtime(ft), Some(expected));
    }

    #[test]
    fn filetime_before_epoch_round_trips_via_checked_sub() {
        // 1969-12-31T23:59:00Z: 60 seconds before the Unix epoch.
        const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
        let ticks = EPOCH_DIFF_100NS - 60 * 10_000_000;
        let ft = FILETIME {
            dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let expected = SystemTime::UNIX_EPOCH - Duration::from_secs(60);
        assert_eq!(filetime_to_systemtime(ft), Some(expected));
    }

    #[test]
    fn filetime_preserves_sub_second_precision() {
        const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
        // 1 second + 1234500 * 100ns = 1.12345s past the epoch.
        let ticks = EPOCH_DIFF_100NS + 10_000_000 + 1_234_500;
        let ft = FILETIME {
            dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let expected = SystemTime::UNIX_EPOCH + Duration::new(1, 123_450_000);
        assert_eq!(filetime_to_systemtime(ft), Some(expected));
    }

    // TODO(windows-ci): `enable_privileges_best_effort`/`enable_privilege`
    // call real token APIs (OpenProcessToken/AdjustTokenPrivileges) and can
    // only be meaningfully exercised on a live Windows session -- both in
    // an elevated context (expect success) and a non-elevated context
    // (expect the ERROR_NOT_ALL_ASSIGNED debug-logged path, and confirm
    // engine construction still succeeds either way).
}
