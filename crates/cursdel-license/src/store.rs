//! Platform-conventional storage locations for the licence file and
//! activation credentials, per the product brief's explicit choices:
//!
//! - Windows: `%LOCALAPPDATA%\RePassCloud\CurseDelete\`
//! - Linux: `~/.config/cursedelete/` (XDG, honouring `XDG_CONFIG_HOME`)
//! - macOS: `~/Library/Application Support/CurseDelete/`
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

fn license_dir() -> PathBuf {
    if let Some(base) = directories::BaseDirs::new() {
        #[cfg(target_os = "windows")]
        {
            return base
                .data_local_dir()
                .join("RePassCloud")
                .join("CurseDelete");
        }
        #[cfg(target_os = "linux")]
        {
            return base.config_dir().join("cursedelete");
        }
        #[cfg(target_os = "macos")]
        {
            return base.data_dir().join("CurseDelete");
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        {
            return base.config_dir().join("cursedelete");
        }
    }
    PathBuf::from(".cursedelete")
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
            .mode(0o600)
            .open(path)?;
        // `OpenOptionsExt::mode` only affects the mode passed to open(2)'s
        // O_CREAT, which the kernel applies solely when this call actually
        // creates the file -- if `path` already existed (e.g. left behind
        // with looser permissions by an older version, a backup/restore
        // tool that doesn't preserve mode bits, or a manual copy), the
        // `mode(0o600)` above is silently ignored and whatever permissions
        // it already had persist across this rewrite. Re-assert 0600
        // explicitly and unconditionally so a secret this file holds
        // (the activation bearer token) is never left readable by other
        // local accounts regardless of the file's prior state.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
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
