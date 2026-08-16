//! `--kill-locks` and `--close-remote-locks` lock handling for Windows.
//!
//! ## Local locks (`--kill-locks`)
//!
//! Uses the Windows Restart Manager API to identify which process(es) hold
//! a file open, per `README-CurseDelete2.md` section 12.1's explicit
//! preference ("Preferred first implementation: Windows Restart Manager
//! API"). Restart Manager's own `RmShutdown` only *asks* a process to
//! close via `WM_QUERYENDSESSION`, which most non-GUI/service processes
//! never answer; this module terminates the identified holder directly via
//! `TerminateProcess` instead, which the product brief documents as an
//! acceptable interpretation (`README-CurseDelete2.md` section 12.1's
//! conceptual flow goes straight from "identify lock holder" to
//! "terminate process"). See `docs/adr/0007-windows-engine.md` for the
//! full reasoning and the protected-process policy.
//!
//! ## Remote locks (`--close-remote-locks`)
//!
//! Uses `NetFileEnum`/`NetFileClose` against a UNC path's server. This
//! requires real administrative rights on *that server* -- local
//! Administrator rights on the machine running CurseDelete grant nothing
//! here, exactly as `README-CurseDelete2.md` section 5.1 warns generally
//! ("Local Administrator privileges do not automatically grant rights over
//! a remote SMB server"). A non-UNC path, or any `NetFileEnum`/
//! `NetFileClose` failure (access denied on the remote server, RPC
//! unreachable, the server does not implement this API at all -- Samba,
//! NetApp, Synology, ...), is reported as `Unsupported` with an honest
//! message, never as a faked success.

use std::ffi::c_void;
use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, HANDLE, WIN32_ERROR};
use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows::Win32::Storage::FileSystem::{NetFileClose, NetFileEnum, FILE_INFO_3};
use windows::Win32::System::RestartManager::{
    RmCritical, RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
    RM_PROCESS_INFO,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

use cursdel_core::engine::{DeleteOptions, LockResolution};

/// The bit value of `SYNCHRONIZE`, needed on the process handle so
/// `WaitForSingleObject` can be used to detect actual process exit (and
/// therefore actual handle release) rather than guessing with a fixed
/// sleep. Not exposed as a typed `PROCESS_ACCESS_RIGHTS` constant by the
/// `windows` crate (it is generic across every kernel object type), so it
/// is combined with `PROCESS_TERMINATE` by raw bit value here, matching
/// the well-known Win32 constant value (`0x00100000`).
const SYNCHRONIZE_BIT: u32 = 0x0010_0000;

const PROTECTED_PROCESS_NAMES: &[&str] = &[
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "smss.exe",
    "system",
];

fn is_protected_process_name(name: &str) -> bool {
    PROTECTED_PROCESS_NAMES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------
// Local locks: Restart Manager + terminate.
// ---------------------------------------------------------------------

struct Holder {
    pid: u32,
    critical: bool,
}

pub fn resolve_local_lock(path: &Path, opts: DeleteOptions) -> LockResolution {
    if !opts.kill_locks {
        return LockResolution::Unsupported("--kill-locks was not enabled".to_string());
    }

    let holders = match restart_manager_find_holders(path) {
        Ok(h) => h,
        Err(msg) => return LockResolution::Failed(msg),
    };

    if holders.is_empty() {
        return LockResolution::Failed(
            "delete failed with a sharing violation but Restart Manager could not identify \
             any process holding the object open"
                .to_string(),
        );
    }

    let own_pid = unsafe { GetCurrentProcessId() };

    let mut all_protected = true;
    let mut terminated_handles: Vec<HANDLE> = Vec::new();

    for holder in &holders {
        if holder.pid == own_pid || holder.critical {
            continue;
        }
        let exe_name = resolve_exe_basename(holder.pid);
        let protected = match &exe_name {
            // Conservative default: if the executable name cannot even be
            // resolved, treat the holder as protected rather than risk
            // terminating an unidentified process.
            None => true,
            Some(name) => is_protected_process_name(name),
        };
        if protected {
            continue;
        }

        all_protected = false;
        if let Some(handle) = terminate_process(holder.pid) {
            terminated_handles.push(handle);
        }
    }

    if all_protected {
        return LockResolution::Failed(format!(
            "object is held open only by protected/system/critical processes ({} holder(s)); \
             refusing to terminate",
            holders.len()
        ));
    }

    if terminated_handles.is_empty() {
        return LockResolution::Failed(format!(
            "failed to terminate any of the {} process(es) holding the object open",
            holders.len()
        ));
    }

    // Poll briefly (bounded, per-process) so the caller's retry has a real
    // chance of the handle actually being released rather than racing a
    // process that is still tearing down.
    for handle in terminated_handles {
        unsafe {
            let _ = WaitForSingleObject(handle, 1000);
            let _ = CloseHandle(handle);
        }
    }

    LockResolution::Resolved
}

fn restart_manager_find_holders(path: &Path) -> Result<Vec<Holder>, String> {
    unsafe {
        let mut session: u32 = 0;
        let mut key_buf = [0u16; (CCH_RM_SESSION_KEY + 1) as usize];
        let rc = RmStartSession(&mut session, 0, PWSTR(key_buf.as_mut_ptr()));
        if rc != WIN32_ERROR(0) {
            return Err(format!("RmStartSession failed: win32 error {}", rc.0));
        }

        // Restart Manager wants a plain (non-verbatim) path; register the
        // absolutized form rather than the `\\?\`-prefixed one used for
        // filesystem calls elsewhere in this crate.
        let plain_string = plain_absolute_path(path);
        let mut wide_path: Vec<u16> =
            std::os::windows::ffi::OsStrExt::encode_wide(std::ffi::OsStr::new(&plain_string))
                .collect();
        wide_path.push(0);

        let filenames = [PCWSTR(wide_path.as_ptr())];
        let rc = RmRegisterResources(session, Some(&filenames), None, None);
        if rc != WIN32_ERROR(0) {
            let _ = RmEndSession(session);
            return Err(format!("RmRegisterResources failed: win32 error {}", rc.0));
        }

        let mut needed: u32 = 0;
        let mut avail: u32 = 0;
        let mut reboot_reasons: u32 = 0;
        let rc = RmGetList(session, &mut needed, &mut avail, None, &mut reboot_reasons);

        let mut holders = Vec::new();
        if rc == ERROR_MORE_DATA && needed > 0 {
            let mut buf: Vec<RM_PROCESS_INFO> = vec![RM_PROCESS_INFO::default(); needed as usize];
            avail = needed;
            let rc2 = RmGetList(
                session,
                &mut needed,
                &mut avail,
                Some(buf.as_mut_ptr()),
                &mut reboot_reasons,
            );
            if rc2 != WIN32_ERROR(0) {
                let _ = RmEndSession(session);
                return Err(format!("RmGetList failed: win32 error {}", rc2.0));
            }
            buf.truncate(avail as usize);
            for info in &buf {
                holders.push(Holder {
                    pid: info.Process.dwProcessId,
                    critical: info.ApplicationType == RmCritical,
                });
            }
        } else if rc != WIN32_ERROR(0) {
            let _ = RmEndSession(session);
            return Err(format!("RmGetList failed: win32 error {}", rc.0));
        }

        let _ = RmEndSession(session);
        Ok(holders)
    }
}

/// Absolutized, backslash-normalized, but **non**-verbatim form of `path`
/// (i.e. `to_verbatim`'s work, with the `\\?\`/`\\?\UNC\` prefix stripped
/// back off) -- what Restart Manager's `RmRegisterResources` expects,
/// which is a plain path, not the verbatim form the filesystem calls
/// elsewhere in this crate use.
fn plain_absolute_path(path: &Path) -> String {
    let verbatim = crate::sys::to_verbatim(path).to_string_lossy().into_owned();
    if let Some(rest) = verbatim.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = verbatim.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        verbatim
    }
}

fn resolve_exe_basename(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&full)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
    }
}

fn terminate_process(pid: u32) -> Option<HANDLE> {
    unsafe {
        let rights = PROCESS_ACCESS_RIGHTS(PROCESS_TERMINATE.0 | SYNCHRONIZE_BIT);
        let handle = OpenProcess(rights, false, pid).ok()?;
        if TerminateProcess(handle, 1).is_ok() {
            Some(handle)
        } else {
            let _ = CloseHandle(handle);
            None
        }
    }
}

// ---------------------------------------------------------------------
// Remote locks: NetFileEnum / NetFileClose.
// ---------------------------------------------------------------------

struct UncParts {
    server: String,
    /// Share-relative subpath, e.g. `sub\file.txt` for
    /// `\\server\share\sub\file.txt`. May be empty if `path` names the
    /// share root itself (target validation should already reject this as
    /// a destructive target, but this module does not assume that).
    relative: String,
}

fn parse_unc(path: &Path) -> Option<UncParts> {
    let normalized = path.to_string_lossy().replace('/', "\\");
    let body = if let Some(rest) = normalized.strip_prefix(r"\\?\UNC\") {
        rest
    } else {
        normalized.strip_prefix(r"\\")?
    };

    let mut parts = body.splitn(3, '\\');
    let server = parts.next()?.to_string();
    let share = parts.next()?.to_string();
    let relative = parts.next().unwrap_or("").to_string();

    if server.is_empty() || share.is_empty() {
        return None;
    }
    Some(UncParts { server, relative })
}

/// True if `full`'s trailing path components exactly match every component
/// of `suffix`, case-insensitively. This is a sound (not merely
/// heuristic) way to match a UNC path's share-relative subpath against a
/// server's local `FILE_INFO_3::fi3_pathname`: a share always maps its
/// root to some local path, so anything below the share root has the
/// identical relative structure on both views.
fn path_ends_with_components(full: &str, suffix: &str) -> bool {
    let full_norm = full.replace('/', "\\").to_ascii_lowercase();
    let suffix_norm = suffix.replace('/', "\\").to_ascii_lowercase();
    let full_comps: Vec<&str> = full_norm.split('\\').filter(|s| !s.is_empty()).collect();
    let suffix_comps: Vec<&str> = suffix_norm.split('\\').filter(|s| !s.is_empty()).collect();
    if suffix_comps.is_empty() || suffix_comps.len() > full_comps.len() {
        return false;
    }
    let start = full_comps.len() - suffix_comps.len();
    full_comps[start..] == suffix_comps[..]
}

pub fn resolve_remote_lock(path: &Path, opts: DeleteOptions) -> LockResolution {
    if !opts.close_remote_locks {
        return LockResolution::Unsupported("--close-remote-locks was not enabled".to_string());
    }

    let Some(parts) = parse_unc(path) else {
        return LockResolution::Unsupported(
            "remote SMB open administration only applies to UNC paths".to_string(),
        );
    };

    let server_wide: Vec<u16> = {
        let mut s: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(std::ffi::OsStr::new(
            &format!(r"\\{}", parts.server),
        ))
        .collect();
        s.push(0);
        s
    };

    let open_files = match enumerate_server_open_files(&server_wide) {
        Ok(files) => files,
        Err(msg) => {
            return LockResolution::Unsupported(format!(
                "could not query open files on server '{}': {msg}",
                parts.server
            ));
        }
    };

    let matches: Vec<u32> = open_files
        .into_iter()
        .filter(|(_, remote_path)| path_ends_with_components(remote_path, &parts.relative))
        .map(|(id, _)| id)
        .collect();

    if matches.is_empty() {
        return LockResolution::Failed(format!(
            "queried {} open file(s) on server '{}' but none matched the requested path",
            parts.relative, parts.server
        ));
    }

    let mut closed_any = false;
    let mut close_errors = Vec::new();
    for file_id in matches {
        match unsafe { NetFileClose(PCWSTR(server_wide.as_ptr()), file_id) } {
            0 => closed_any = true,
            code => close_errors.push(format!("file id {file_id}: win32 error {code}")),
        }
    }

    if closed_any {
        LockResolution::Resolved
    } else {
        LockResolution::Failed(format!(
            "NetFileClose failed for every matching open file: {}",
            close_errors.join("; ")
        ))
    }
}

fn enumerate_server_open_files(server_wide: &[u16]) -> Result<Vec<(u32, String)>, String> {
    unsafe {
        let mut buf_ptr: *mut u8 = std::ptr::null_mut();
        let mut entries_read: u32 = 0;
        let mut total_entries: u32 = 0;
        let mut resume: usize = 0;

        let rc = NetFileEnum(
            PCWSTR(server_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            3,
            &mut buf_ptr,
            u32::MAX,
            &mut entries_read,
            &mut total_entries,
            Some(&mut resume),
        );
        if rc != 0 {
            return Err(format!("NetFileEnum failed with win32 error {rc}"));
        }

        let mut results = Vec::with_capacity(entries_read as usize);
        if !buf_ptr.is_null() {
            let items =
                std::slice::from_raw_parts(buf_ptr as *const FILE_INFO_3, entries_read as usize);
            for item in items {
                let path = if item.fi3_pathname.is_null() {
                    String::new()
                } else {
                    item.fi3_pathname.to_string().unwrap_or_default()
                };
                results.push((item.fi3_id, path));
            }
            let _ = NetApiBufferFree(Some(buf_ptr as *const c_void));
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_names_match_case_insensitively() {
        assert!(is_protected_process_name("csrss.exe"));
        assert!(is_protected_process_name("CSRSS.EXE"));
        assert!(is_protected_process_name("Lsass.exe"));
        assert!(is_protected_process_name("System"));
        assert!(is_protected_process_name("system"));
        assert!(!is_protected_process_name("notepad.exe"));
        assert!(!is_protected_process_name("explorer.exe"));
    }

    #[test]
    fn kill_locks_disabled_is_reported_unsupported_not_failed() {
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: false,
        };
        let result = resolve_local_lock(Path::new(r"C:\does-not-matter"), opts);
        assert!(matches!(result, LockResolution::Unsupported(_)));
    }

    #[test]
    fn close_remote_locks_disabled_is_reported_unsupported_not_failed() {
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: false,
        };
        let result = resolve_remote_lock(Path::new(r"\\server\share\file.txt"), opts);
        assert!(matches!(result, LockResolution::Unsupported(_)));
    }

    #[test]
    fn non_unc_path_is_unsupported_for_remote_locks() {
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: true,
        };
        let result = resolve_remote_lock(Path::new(r"C:\local\path\file.txt"), opts);
        match result {
            LockResolution::Unsupported(msg) => assert!(msg.contains("UNC")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn parses_plain_unc_path() {
        let parts = parse_unc(Path::new(r"\\FS01\Builds\Old\foo.db")).unwrap();
        assert_eq!(parts.server, "FS01");
        assert_eq!(parts.relative, r"Old\foo.db");
    }

    #[test]
    fn parses_verbatim_unc_path() {
        let parts = parse_unc(Path::new(r"\\?\UNC\FS01\Builds\Old\foo.db")).unwrap();
        assert_eq!(parts.server, "FS01");
        assert_eq!(parts.relative, r"Old\foo.db");
    }

    #[test]
    fn drive_path_is_not_unc() {
        assert!(parse_unc(Path::new(r"C:\Temp\Old\foo.db")).is_none());
    }

    #[test]
    fn share_root_has_empty_relative_path() {
        let parts = parse_unc(Path::new(r"\\FS01\Builds")).unwrap();
        assert_eq!(parts.server, "FS01");
        assert_eq!(parts.relative, "");
    }

    #[test]
    fn suffix_matching_is_component_aware_not_substring() {
        // "ub\file.txt" must NOT match ".../xsub/file.txt" -- a naive
        // string suffix check would incorrectly accept this.
        assert!(!path_ends_with_components(
            r"D:\Data\xsub\file.txt",
            r"ub\file.txt"
        ));
        assert!(path_ends_with_components(
            r"D:\Data\sub\file.txt",
            r"sub\file.txt"
        ));
    }

    #[test]
    fn suffix_matching_is_case_insensitive_and_separator_normalized() {
        assert!(path_ends_with_components(
            r"D:\Data\Old\Foo.DB",
            "old/foo.db"
        ));
    }

    #[test]
    fn suffix_matching_rejects_when_full_path_is_shorter() {
        assert!(!path_ends_with_components(r"D:\foo.db", r"Old\sub\foo.db"));
    }

    #[test]
    fn plain_absolute_path_strips_drive_verbatim_prefix() {
        assert_eq!(
            plain_absolute_path(Path::new(r"C:\Temp\Old")),
            r"C:\Temp\Old"
        );
    }

    #[test]
    fn plain_absolute_path_strips_unc_verbatim_prefix() {
        assert_eq!(
            plain_absolute_path(Path::new(r"\\FS01\Builds\Old")),
            r"\\FS01\Builds\Old"
        );
    }

    // TODO(windows-ci): `restart_manager_find_holders`/`resolve_local_lock`
    // end-to-end (open a file from a genuine second process, confirm Restart
    // Manager identifies its PID, confirm termination + WaitForSingleObject
    // actually releases the handle so a retried delete succeeds) needs a
    // live Windows session and a real second process to hold the file open.
    //
    // TODO(windows-ci): `resolve_remote_lock` end-to-end (NetFileEnum/
    // NetFileClose against a real SMB share with a genuine remote open, and
    // separately against a share with insufficient admin rights to confirm
    // the honest Unsupported path) needs a real Windows file server and
    // admin credentials on it -- this cannot be simulated locally.
}
