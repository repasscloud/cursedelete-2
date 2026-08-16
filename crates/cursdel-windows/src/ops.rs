//! Delete and remediation operations for the Windows engine.
//!
//! Every delete opens the target **object itself** (never its parent, and
//! never by re-resolving a path string a second time after listing) with
//! `FILE_FLAG_OPEN_REPARSE_POINT` set unconditionally, so a reparse point
//! at the final path component is opened as itself and never followed --
//! whether or not it actually is one. See
//! `docs/adr/0007-windows-engine.md` for exactly what this does and does
//! not protect against.
//!
//! The primary delete path is `CreateFileW` + `SetFileInformationByHandle`
//! (`FileDispositionInfoEx`, `FILE_DISPOSITION_FLAG_DELETE |
//! FILE_DISPOSITION_FLAG_POSIX_SEMANTICS |
//! FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE`), falling back to the
//! classic `DeleteFileW`/`RemoveDirectoryW` when that is unavailable or
//! fails. Unlike `_old/sfvdd`'s fallback (which discards the fallback
//! call's own result with `let _ = ...`), this module always reports
//! whatever the fallback actually returned.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;

use windows::core::{Error as WinError, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    CopySid, GetLengthSid, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
    NO_INHERITANCE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DeleteFileW, FileDispositionInfoEx, GetFileAttributesW, RemoveDirectoryW,
    SetFileAttributesW, SetFileInformationByHandle, DELETE, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_READONLY, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO_EX, FILE_DISPOSITION_INFO_EX_FLAGS, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use cursdel_core::engine::{DeleteAttempt, DeleteOptions, RemediationOutcome};
use cursdel_core::error::{DeleteFailure, FailureCategory};

use crate::errmap;
use crate::sys::to_wide_verbatim;

pub fn delete_file(path: &Path, _opts: DeleteOptions) -> DeleteAttempt {
    delete_object(path, false)
}

pub fn delete_dir(path: &Path, _opts: DeleteOptions) -> DeleteAttempt {
    delete_object(path, true)
}

fn delete_object(path: &Path, as_dir: bool) -> DeleteAttempt {
    let wide = to_wide_verbatim(path);

    match disposition_delete(&wide, as_dir) {
        Ok(()) => return DeleteAttempt::deleted(0),
        Err(DispositionFailure::Open(e)) if errmap::is_not_found(&e) => {
            return DeleteAttempt::already_gone();
        }
        Err(primary_error) => {
            // Fall through to the classic fallback below. Deliberately not
            // matched on a specific error code here: the handle-based path
            // can fail to open for the same reasons the classic API would
            // (in which case the fallback fails identically and reports an
            // equally accurate error), or fail for a reason specific to
            // this newer API (unsupported by the filesystem driver, etc),
            // in which case the fallback is precisely the right thing to
            // try next. The primary path's own error is only kept around
            // for diagnostics -- the fallback's result (success or
            // failure) is always what gets reported to the caller.
            tracing::trace!(
                path = %path.display(),
                reason = %primary_error.into_inner(),
                "primary FILE_DISPOSITION_INFO_EX delete path failed; trying classic \
                 DeleteFileW/RemoveDirectoryW fallback"
            );
        }
    }

    match classic_delete(&wide, as_dir) {
        Ok(()) => DeleteAttempt::deleted(0),
        Err(e) if errmap::is_not_found(&e) => DeleteAttempt::already_gone(),
        Err(e) => DeleteAttempt::failed(make_failure(path, as_dir, &e)),
    }
}

enum DispositionFailure {
    Open(WinError),
    SetDisposition(WinError),
}

impl DispositionFailure {
    fn into_inner(self) -> WinError {
        match self {
            DispositionFailure::Open(e) | DispositionFailure::SetDisposition(e) => e,
        }
    }
}

/// The modern delete path: open the object itself, then mark it for
/// deletion via `FileDispositionInfoEx`. `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS`
/// removes the directory entry immediately rather than deferring to last
/// handle close, matching the semantics every other part of this pipeline
/// assumes (a delete either succeeded or it didn't -- not "succeeded, but
/// the name may still be visible for a while"). `FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE`
/// means a plain, non-force-mode delete already succeeds against a
/// read-only file without a separate attribute-clearing round trip -- see
/// `docs/adr/0007-windows-engine.md`.
fn disposition_delete(wide_path: &[u16], as_dir: bool) -> Result<(), DispositionFailure> {
    let desired = (DELETE | FILE_READ_ATTRIBUTES).0;
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    // FILE_FLAG_OPEN_REPARSE_POINT is unconditional: harmless when the
    // target is not a reparse point, and the only thing standing between
    // "delete the link" and "silently follow the link" when it is one.
    let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
    if as_dir {
        // Required to obtain a handle to a directory at all.
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }

    let handle: HANDLE = unsafe {
        CreateFileW(
            PCWSTR(wide_path.as_ptr()),
            desired,
            share,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    }
    .map_err(DispositionFailure::Open)?;

    let mut info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_INFO_EX_FLAGS(
            FILE_DISPOSITION_FLAG_DELETE.0
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE.0,
        ),
    };
    let result = unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfoEx,
            &mut info as *mut _ as *const c_void,
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.map_err(DispositionFailure::SetDisposition)
}

/// Classic fallback. Also opens (implicitly, inside `DeleteFileW`/
/// `RemoveDirectoryW`) the object by its verbatim path rather than a
/// non-prefixed one, so long-path/UNC support is preserved on this path
/// too.
fn classic_delete(wide_path: &[u16], as_dir: bool) -> windows::core::Result<()> {
    if as_dir {
        unsafe { RemoveDirectoryW(PCWSTR(wide_path.as_ptr())) }
    } else {
        unsafe { DeleteFileW(PCWSTR(wide_path.as_ptr())) }
    }
}

fn make_failure(path: &Path, is_dir: bool, err: &WinError) -> DeleteFailure {
    let code = errmap::win32_code(err);
    DeleteFailure {
        path: path.to_path_buf(),
        is_directory: is_dir,
        category: errmap::categorize_error(err),
        message: err.to_string(),
        os_error_code: code.map(|c| c as i32),
    }
}

/// Attribute/ownership/ACL remediation, only ever invoked (by
/// `cursdel-core`'s pipeline) after a delete has already failed with a
/// remediable category. Because the pipeline calls this once and retries
/// the delete exactly once afterward (see `crate::lib` module docs and
/// `docs/adr/0007-windows-engine.md`), both remediation steps -- clearing
/// the read-only attribute, and taking ownership + granting the current
/// token's user full control -- are attempted together in a single call
/// rather than gated on each other's success; this is safe because it
/// only ever runs on the already-failed path (never the normal
/// successful-delete path), and each step is independently a no-op when
/// it has nothing to fix or no authority to fix it.
pub fn remediate(
    path: &Path,
    _is_dir: bool,
    failure: &DeleteFailure,
    opts: DeleteOptions,
) -> RemediationOutcome {
    if !opts.allow_remediation {
        return RemediationOutcome::NotApplicable;
    }
    if !matches!(
        failure.category,
        FailureCategory::AccessDenied | FailureCategory::ReparsePointRefused
    ) {
        return RemediationOutcome::NotApplicable;
    }

    let mut applied = false;
    let mut last_error: Option<String> = None;

    if clear_readonly_attribute(path) {
        applied = true;
    }

    match take_ownership_and_grant_current_user(path) {
        Ok(()) => applied = true,
        Err(message) => last_error = Some(message),
    }

    if applied {
        RemediationOutcome::Applied
    } else {
        RemediationOutcome::Failed(last_error.unwrap_or_else(|| {
            "no attribute, ownership, or ACL change could be applied with the current \
             security context's authority"
                .to_string()
        }))
    }
}

/// Clears `FILE_ATTRIBUTE_READONLY` if set. Deliberately does **not** touch
/// hidden/system attributes: neither actually blocks Win32 deletion (only
/// read-only does), so clearing them would be an unrequested, unnecessary
/// side effect on an object that -- if remediation as a whole fails for an
/// unrelated ACL reason -- might survive the operation with attributes
/// altered for no benefit.
fn clear_readonly_attribute(path: &Path) -> bool {
    let wide = to_wide_verbatim(path);
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return false;
    }
    if attrs & FILE_ATTRIBUTE_READONLY.0 == 0 {
        return false;
    }
    let mut new_attrs = attrs & !FILE_ATTRIBUTE_READONLY.0;
    if new_attrs == 0 {
        new_attrs = FILE_ATTRIBUTE_NORMAL.0;
    }
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(new_attrs)) }
        .is_ok()
}

/// An owned copy of a SID's raw bytes, so it outlives the token-information
/// buffer it was originally read from.
struct OwnedSid(Vec<u8>);

impl OwnedSid {
    fn as_psid(&self) -> PSID {
        PSID(self.0.as_ptr() as *mut c_void)
    }
}

fn current_process_user_sid() -> Result<OwnedSid, String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| e.to_string())?;

        let mut len: u32 = 0;
        // Expected to fail with ERROR_INSUFFICIENT_BUFFER; `len` is still
        // written with the required size, per the Win32 contract.
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        if len == 0 {
            let _ = CloseHandle(token);
            return Err("GetTokenInformation did not report a required buffer size".to_string());
        }

        let mut buf = vec![0u8; len as usize];
        let result = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        result.map_err(|e| e.to_string())?;

        let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid_ptr = token_user.User.Sid;
        let sid_len = GetLengthSid(sid_ptr);
        if sid_len == 0 {
            return Err("current process token's user SID has zero length".to_string());
        }

        let mut owned = vec![0u8; sid_len as usize];
        CopySid(sid_len, PSID(owned.as_mut_ptr() as *mut c_void), sid_ptr)
            .map_err(|e| e.to_string())?;
        Ok(OwnedSid(owned))
    }
}

/// Takes ownership of `path` and grants the *current process token's
/// user* -- never an unconditional `BUILTIN\Administrators` -- full
/// control, in a single `SetNamedSecurityInfoW` call. This does exactly
/// what `README-CurseDelete2.md` section 5.1 requires and nothing more:
/// if the current token does not actually hold `SeTakeOwnershipPrivilege`/
/// `WRITE_OWNER`/`WRITE_DAC` rights over the object, `SetEntriesInAclW`/
/// `SetNamedSecurityInfoW` simply fail (the OS enforces this boundary
/// itself; this function does not attempt to detect or work around that,
/// it just reports the OS's answer honestly via `Err`).
fn take_ownership_and_grant_current_user(path: &Path) -> Result<(), String> {
    let sid = current_process_user_sid()?;

    unsafe {
        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(sid.as_psid().0 as *mut u16),
        };

        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: windows::Win32::Foundation::GENERIC_ALL.0,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: trustee,
        };

        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let rc = SetEntriesInAclW(Some(&[ea]), None, &mut new_dacl);
        if rc.0 != 0 {
            return Err(format!("SetEntriesInAclW failed: win32 error {}", rc.0));
        }

        let wide = to_wide_verbatim(path);
        let hr = SetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            sid.as_psid(),
            PSID(std::ptr::null_mut()),
            Some(new_dacl as *const ACL),
            None,
        );

        if !new_dacl.is_null() {
            let _ = windows::Win32::Foundation::LocalFree(HLOCAL(new_dacl as *mut c_void));
        }

        if hr.0 != 0 {
            return Err(format!(
                "SetNamedSecurityInfoW failed: win32 error {}",
                hr.0
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cursdel_core::engine::DeleteOutcome;

    fn default_opts() -> DeleteOptions {
        DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: false,
        }
    }

    #[test]
    fn deletes_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let attempt = delete_file(&file, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
        assert!(!file.exists());
    }

    #[test]
    fn deletes_an_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let attempt = delete_dir(&sub, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
        assert!(!sub.exists());
    }

    #[test]
    fn deleting_nonempty_directory_fails_with_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("f.txt"), b"x").unwrap();
        let attempt = delete_dir(&sub, default_opts());
        match attempt.outcome {
            DeleteOutcome::Failed(f) => assert_eq!(f.category, FailureCategory::NotEmpty),
            other => panic!("expected NotEmpty failure, got {other:?}"),
        }
    }

    #[test]
    fn deleting_already_gone_file_reports_already_gone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gone.txt");
        // Never created.
        let attempt = delete_file(&file, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::AlreadyGone));
    }

    #[test]
    fn deletes_a_readonly_file_without_remediation() {
        // The primary FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE path
        // means a plain delete already succeeds against a read-only file;
        // remediation should not even be necessary for this case.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ro.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms).unwrap();

        let attempt = delete_file(&file, default_opts());
        assert!(matches!(attempt.outcome, DeleteOutcome::Deleted));
        assert!(!file.exists());
    }

    #[test]
    fn remediate_reports_not_applicable_when_remediation_disallowed() {
        let failure = DeleteFailure {
            path: std::path::PathBuf::from(r"C:\doesnotmatter"),
            is_directory: false,
            category: FailureCategory::AccessDenied,
            message: "simulated".to_string(),
            os_error_code: None,
        };
        let opts = DeleteOptions {
            allow_remediation: false,
            kill_locks: false,
            close_remote_locks: false,
        };
        let outcome = remediate(
            std::path::Path::new(r"C:\doesnotmatter"),
            false,
            &failure,
            opts,
        );
        assert!(matches!(outcome, RemediationOutcome::NotApplicable));
    }

    #[test]
    fn remediate_reports_not_applicable_for_non_remediable_category() {
        let failure = DeleteFailure {
            path: std::path::PathBuf::from(r"C:\doesnotmatter"),
            is_directory: false,
            category: FailureCategory::NotEmpty,
            message: "simulated".to_string(),
            os_error_code: None,
        };
        let opts = DeleteOptions {
            allow_remediation: true,
            kill_locks: false,
            close_remote_locks: false,
        };
        let outcome = remediate(
            std::path::Path::new(r"C:\doesnotmatter"),
            false,
            &failure,
            opts,
        );
        assert!(matches!(outcome, RemediationOutcome::NotApplicable));
    }

    #[test]
    fn remediate_clears_readonly_attribute_and_reports_applied() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ro.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms).unwrap();

        let failure = DeleteFailure {
            path: file.clone(),
            is_directory: false,
            category: FailureCategory::AccessDenied,
            message: "simulated".to_string(),
            os_error_code: None,
        };
        let opts = DeleteOptions {
            allow_remediation: true,
            kill_locks: false,
            close_remote_locks: false,
        };
        let outcome = remediate(&file, false, &failure, opts);
        assert!(matches!(outcome, RemediationOutcome::Applied));

        let attrs = std::fs::metadata(&file).unwrap().permissions();
        assert!(!attrs.readonly());
    }

    // TODO(windows-ci): the ownership/ACL branch of `remediate`
    // (`take_ownership_and_grant_current_user`) needs a real restrictive
    // DACL that denies the current token DELETE/WRITE_DAC rights outright
    // (constructed via `icacls`/`SetNamedSecurityInfoW` in test setup) to
    // exercise the "actually changes ownership and unblocks a retry" path;
    // it also needs both an elevated and a non-elevated session to confirm
    // the honest-failure behaviour when the token genuinely lacks
    // authority (see docs/adr/0007-windows-engine.md).
    //
    // TODO(windows-ci): deleting a reparse point (junction / directory
    // symlink / file symlink) without following it -- proving
    // FILE_FLAG_OPEN_REPARSE_POINT actually removes the link object and
    // leaves its target untouched -- needs `std::os::windows::fs::symlink_dir`/
    // `symlink_file`, which typically requires either an elevated session
    // or Developer Mode enabled on the runner.
}
