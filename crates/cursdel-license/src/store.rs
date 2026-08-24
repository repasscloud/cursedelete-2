//! Platform-conventional storage locations for the licence file and
//! activation credentials:
//!
//! - Windows: `%LOCALAPPDATA%\RePassCloud\CurseDelete-2\`
//! - Linux: `~/.config/cursedelete-2/` (XDG, honouring `XDG_CONFIG_HOME`)
//! - macOS: `~/Library/Application Support/CurseDelete-2/`
//!
//! Named `CurseDelete-2` (matching this repository, `cursedelete-2`, and
//! the product's "CurseDelete 2" branding -- see `README.md`'s Release
//! Philosophy section) rather than plain `CurseDelete`, so this crate's
//! licence state can never collide with a machine that also has the
//! separate, pre-rewrite C# CurseDelete installed. Any machine that
//! activated under this crate's earlier, unqualified `CurseDelete` path
//! (v2.0.0's original location) needs to re-activate/re-enroll once on
//! this path -- deliberate, since the product has no wide install base
//! yet and disambiguating the two products' state going forward matters
//! more than preserving that short-lived path.
//!
//! The activation token is a bearer credential (see
//! `LICENSING-INTEGRATION.md` §6.1: "treat it like a password"). It is
//! stored in a file created with owner-only permissions (`0600` on Unix)
//! rather than an OS keychain -- see `docs/adr/0004-licensing-integration.md`
//! for why: keychain access on some platforms (notably macOS Keychain for
//! an unsigned/ad-hoc-signed binary) can require interactive user consent,
//! which would break the product's explicit requirement for unattended/
//! CI/scheduled automation (Business/Enterprise editions). This mirrors
//! the common practice of mature automation-friendly CLIs (`aws`, `gcloud`,
//! `gh`) that default to a protected local file rather than a keychain.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct LicensePaths {
    pub license_file: PathBuf,
    pub activation_credentials_file: PathBuf,
}

pub fn default_paths() -> LicensePaths {
    let dir = license_dir();
    LicensePaths {
        license_file: dir.join("license.json"),
        activation_credentials_file: dir.join("activation.json"),
    }
}

/// Machine-wide storage paths, used by Deployment Key enrollment
/// (`cursdel license enroll`). Unlike [`default_paths`] (per-user), these
/// locations are shared by every account on the machine, so any user
/// running `cursdel` on an enrolled machine resolves the same license --
/// the point of unattended, seat-aware machine enrollment.
pub fn machine_wide_paths() -> LicensePaths {
    let dir = machine_wide_dir();
    LicensePaths {
        license_file: dir.join("license.json"),
        activation_credentials_file: dir.join("activation.json"),
    }
}

/// Overrides [`machine_wide_dir`]'s real, OS-owned location (`/var/lib/...`,
/// `%PROGRAMDATA%\...`, `/Library/Application Support/...`) when set.
/// Exists solely so tests -- and the CLI integration tests in particular,
/// which spawn the real binary -- can exercise Deployment Key enrollment
/// against an isolated temporary directory instead of real, root-owned
/// machine state (which would be both unwritable by an unprivileged test
/// runner and actively wrong to mutate from a test). Not documented as a
/// user-facing configuration knob, and -- since an unprivileged caller
/// could otherwise point this at a directory it already owns to make
/// `enroll` report machine-wide success while no other account can
/// actually see the result, bypassing the whole point of the
/// administrator/root preflight in [`ensure_machine_wide_writable`] --
/// compiled in only for `#[cfg(debug_assertions)]` builds (`cargo build`/
/// `cargo test` without `--release`), never for the `--release` profile
/// this product actually ships (see `[profile.release]` in the workspace
/// `Cargo.toml` and `docs/RELEASE.md`).
#[cfg(debug_assertions)]
const MACHINE_DIR_OVERRIDE_ENV_VAR: &str = "CURSDEL_MACHINE_LICENSE_DIR_FOR_TESTS";

// Each `#[cfg(...)]` block below is the sole surviving statement in its
// respective per-target build (the others are stripped before clippy ever
// sees them), which is what makes clippy flag `return` as needless on
// whichever single target compiled it -- keeping every branch's `return`
// symmetrical here is clearer than special-casing the one that happens to
// be last.
#[allow(clippy::needless_return)]
fn machine_wide_dir() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os(MACHINE_DIR_OVERRIDE_ENV_VAR) {
        return PathBuf::from(dir);
    }

    #[cfg(target_os = "windows")]
    {
        let program_data = std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
        return program_data.join("CurseDelete-2");
    }
    #[cfg(target_os = "macos")]
    {
        return PathBuf::from("/Library/Application Support/CurseDelete-2");
    }
    #[cfg(target_os = "linux")]
    {
        return PathBuf::from("/var/lib/cursedelete-2");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        PathBuf::from("/var/lib/cursedelete-2")
    }
}

/// Fails clearly, *before* any network call, if the current process lacks
/// the privilege to establish machine-wide state -- Deployment Key
/// enrollment must never silently fall back to user-scoped activation
/// (see the enrollment behaviour contract). Creates the machine-wide
/// directory (if missing) and probes it with a throwaway file.
pub fn ensure_machine_wide_writable(paths: &LicensePaths) -> io::Result<()> {
    let dir = paths
        .license_file
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid machine-wide path"))?;
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".cursdel-write-check");
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)?;
    Ok(())
}

/// Saves the (public, non-secret) signed license envelope to a
/// machine-wide location, world-readable so any local user account can
/// resolve the machine's license, but writable only by whatever privilege
/// level created it (administrator/root on all three platforms).
pub fn save_machine_license_file(
    paths: &LicensePaths,
    signed_license_json: &str,
) -> io::Result<()> {
    write_with_mode(&paths.license_file, signed_license_json.as_bytes(), 0o644)
}

/// Saves activation credentials (a bearer token) to a machine-wide
/// location, owner-only -- same secrecy requirement as the per-user
/// activation file, just rooted under the machine-wide directory instead.
///
/// Unlike the per-user path (which inherits the profile's already
/// owner-restricted NTFS ACLs), `%PROGRAMDATA%` is normally readable by
/// every local account -- `write_with_mode`'s Unix `0600` argument has no
/// Windows equivalent via a plain file write, so on Windows this also
/// applies an explicit ACL restricting the file to Administrators and
/// SYSTEM after writing it, closing what would otherwise be a real gap: a
/// non-admin local user reading the machine's bearer token straight out
/// of a world-readable ProgramData file and using it to call the licence
/// server's `deactivate`/`refresh` endpoints as this machine.
pub fn save_machine_activation_credentials(
    paths: &LicensePaths,
    creds: &ActivationCredentials,
) -> io::Result<()> {
    let json = serde_json::to_string_pretty(creds)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_with_mode(&paths.activation_credentials_file, json.as_bytes(), 0o600)?;
    #[cfg(target_os = "windows")]
    restrict_to_administrators_and_system(&paths.activation_credentials_file)?;
    Ok(())
}

/// Removes inherited ACEs and grants Full Control to only BUILTIN\Administrators
/// (`S-1-5-32-544`) and SYSTEM (`S-1-5-18`) on `path`, via the `icacls`
/// tool that ships with every supported Windows version. Well-known SIDs
/// are used (rather than the `Administrators`/`SYSTEM` names) to avoid
/// localisation issues, mirroring the same well-known-SID practice already
/// used for the Windows installer's own ACL handling (see
/// `build/windows/wix/Package.wxs`).
#[cfg(target_os = "windows")]
fn restrict_to_administrators_and_system(path: &Path) -> io::Result<()> {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg("*S-1-5-32-544:F")
        .arg("/grant:r")
        .arg("*S-1-5-18:F")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "icacls failed to restrict permissions on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

fn license_dir() -> PathBuf {
    if let Some(base) = directories::BaseDirs::new() {
        #[cfg(target_os = "windows")]
        {
            return base
                .data_local_dir()
                .join("RePassCloud")
                .join("CurseDelete-2");
        }
        #[cfg(target_os = "linux")]
        {
            return base.config_dir().join("cursedelete-2");
        }
        #[cfg(target_os = "macos")]
        {
            return base.data_dir().join("CurseDelete-2");
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        {
            return base.config_dir().join("cursedelete-2");
        }
    }
    PathBuf::from(".cursedelete-2")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationCredentials {
    #[serde(rename = "activationId")]
    pub activation_id: String,
    #[serde(rename = "activationToken")]
    pub activation_token: String,
    pub mode: String,
}

pub fn save_license_file(paths: &LicensePaths, signed_license_json: &str) -> io::Result<()> {
    write_restrictive(&paths.license_file, signed_license_json.as_bytes())
}

pub fn load_license_file(paths: &LicensePaths) -> io::Result<Option<String>> {
    match std::fs::read_to_string(&paths.license_file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_activation_credentials(
    paths: &LicensePaths,
    creds: &ActivationCredentials,
) -> io::Result<()> {
    let json = serde_json::to_string_pretty(creds)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_restrictive(&paths.activation_credentials_file, json.as_bytes())
}

pub fn load_activation_credentials(
    paths: &LicensePaths,
) -> io::Result<Option<ActivationCredentials>> {
    match std::fs::read_to_string(&paths.activation_credentials_file) {
        Ok(text) => {
            let creds = serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(creds))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn remove_activation_credentials(paths: &LicensePaths) -> io::Result<()> {
    match std::fs::remove_file(&paths.activation_credentials_file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn remove_license_file(paths: &LicensePaths) -> io::Result<()> {
    match std::fs::remove_file(&paths.license_file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn write_restrictive(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with_mode(path, bytes, 0o600)
}

fn write_with_mode(
    path: &Path,
    bytes: &[u8],
    #[cfg_attr(not(unix), allow(unused_variables))] mode: u32,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(path)?;
        // `OpenOptionsExt::mode` only affects the mode passed to open(2)'s
        // O_CREAT, which the kernel applies solely when this call actually
        // creates the file -- if `path` already existed (e.g. left behind
        // with looser permissions by an older version, a backup/restore
        // tool that doesn't preserve mode bits, or a manual copy), the
        // `mode(mode)` above is silently ignored and whatever permissions
        // it already had persist across this rewrite. Re-assert the
        // intended mode explicitly and unconditionally so a secret this
        // file may hold (an activation bearer token) is never left
        // readable by other local accounts regardless of the file's prior
        // state.
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.write_all(bytes)
    }

    #[cfg(not(unix))]
    {
        // Windows: the destination lives under the current user's
        // %LOCALAPPDATA%, which inherits owner-only-by-default NTFS ACLs
        // from the user profile. Explicit ACL hardening (mirroring the
        // Windows engine's ownership/ACL code) is a documented follow-up
        // if a security review calls for defense-in-depth beyond profile
        // inheritance.
        std::fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_license_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LicensePaths {
            license_file: dir.path().join("license.json"),
            activation_credentials_file: dir.path().join("activation.json"),
        };
        save_license_file(&paths, "{\"hello\":true}").unwrap();
        let loaded = load_license_file(&paths).unwrap();
        assert_eq!(loaded, Some("{\"hello\":true}".to_string()));
    }

    #[test]
    fn missing_license_file_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LicensePaths {
            license_file: dir.path().join("license.json"),
            activation_credentials_file: dir.path().join("activation.json"),
        };
        assert_eq!(load_license_file(&paths).unwrap(), None);
    }

    #[test]
    fn round_trips_activation_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LicensePaths {
            license_file: dir.path().join("license.json"),
            activation_credentials_file: dir.path().join("activation.json"),
        };
        let creds = ActivationCredentials {
            activation_id: "act-1".to_string(),
            activation_token: "secret-token".to_string(),
            mode: "online".to_string(),
        };
        save_activation_credentials(&paths, &creds).unwrap();
        let loaded = load_activation_credentials(&paths).unwrap().unwrap();
        assert_eq!(loaded.activation_id, "act-1");
        assert_eq!(loaded.activation_token, "secret-token");
    }

    #[cfg(unix)]
    #[test]
    fn activation_credentials_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = LicensePaths {
            license_file: dir.path().join("license.json"),
            activation_credentials_file: dir.path().join("activation.json"),
        };
        save_activation_credentials(
            &paths,
            &ActivationCredentials {
                activation_id: "a".to_string(),
                activation_token: "b".to_string(),
                mode: "online".to_string(),
            },
        )
        .unwrap();
        let mode = std::fs::metadata(&paths.activation_credentials_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// Regression test for a real gap found in security review: POSIX's
    /// `open(..., O_CREAT, mode)` only applies `mode` when it actually
    /// creates the file. Writing to a file that already exists with looser
    /// permissions (left behind by an older version, a backup/restore
    /// tool, or a manual copy) must not silently leave those permissions
    /// in place on a file holding a bearer credential.
    #[cfg(unix)]
    #[test]
    fn activation_credentials_permissions_are_restored_on_a_preexisting_loose_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = LicensePaths {
            license_file: dir.path().join("license.json"),
            activation_credentials_file: dir.path().join("activation.json"),
        };

        // Simulate a pre-existing file with world-readable permissions,
        // as if left behind by an older version or a backup/restore tool.
        std::fs::write(&paths.activation_credentials_file, b"stale").unwrap();
        std::fs::set_permissions(
            &paths.activation_credentials_file,
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        save_activation_credentials(
            &paths,
            &ActivationCredentials {
                activation_id: "a".to_string(),
                activation_token: "top-secret".to_string(),
                mode: "online".to_string(),
            },
        )
        .unwrap();

        let mode = std::fs::metadata(&paths.activation_credentials_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "permissions must be reasserted even when the file already existed"
        );
    }

    #[test]
    fn default_paths_are_non_empty_and_platform_specific() {
        let paths = default_paths();
        assert!(
            paths.license_file.to_string_lossy().contains("CurseDelete")
                || paths.license_file.to_string_lossy().contains("cursedelete")
        );
    }
}
