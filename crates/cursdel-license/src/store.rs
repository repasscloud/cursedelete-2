//! Where licences and Business activation state live on disk.
//!
//! Each major version has its own directories, so v2 and v3 of the same
//! application installed side by side never overwrite each other's
//! licences. Candidate licences are read, in precedence order, from:
//!
//! 1. an explicit file named by the application's `*_LICENSE_FILE`
//!    environment variable (installer / automation / admin override);
//! 2. the per-user licence written by `install`/`activate`/`renew`;
//! 3. every `*.license` / `*.json` file in the administrator-managed
//!    directory (the platform default below, or the directory named by the
//!    application's `*_LICENSE_DIR` environment variable). The application
//!    only ever reads from here.
//!
//! | Platform | Per-user directory | Administrator directory |
//! |---|---|---|
//! | Windows | `%LOCALAPPDATA%\<Vendor>\<App>\v<N>` | `%ProgramData%\<Vendor>\<App>\v<N>` |
//! | macOS | `~/Library/Application Support/<App>/v<N>` | `/Library/Application Support/<App>/v<N>` |
//! | Linux | `$XDG_CONFIG_HOME/<app>/v<N>` | `/etc/<app>/v<N>` |
//!
//! Business activation state (the signed certificate and the client
//! secret used to renew/deactivate it) is per-user, alongside the user
//! licence. The client secret is written owner-only (`0600` on Unix).
//! Every write goes to a temporary file first and is renamed into place,
//! so a crash or full disk never leaves a half-written licence.

use std::io;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::resolve::Candidate;

/// Per-application naming used to derive storage locations.
#[derive(Debug, Clone, Copy)]
pub struct StorageNames {
    /// Windows vendor directory, e.g. `RePassCloud`.
    pub vendor: &'static str,
    /// Display-cased application directory (Windows/macOS), e.g. `CurseDelete`.
    pub app: &'static str,
    /// Lowercase application directory (Linux), e.g. `cursedelete`.
    pub unix_app: &'static str,
    /// Environment variable naming one explicit licence file.
    pub license_file_env: &'static str,
    /// Environment variable replacing the administrator directory.
    pub license_dir_env: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseLocations {
    pub override_file: Option<PathBuf>,
    pub user_dir: PathBuf,
    pub admin_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationCredentials {
    pub activation_id: String,
    /// Bearer secret proving this installation owns the activation.
    /// Generated locally, sent only to the licensing server, never printed.
    pub client_secret: String,
}

const LICENSE_FILE: &str = "license.json";
const ACTIVATION_FILE: &str = "activation.json";
const CREDENTIALS_FILE: &str = "credentials.json";

impl LicenseLocations {
    /// The platform locations for `names` and `major_version`, honouring
    /// the application's override environment variables.
    pub fn for_app(names: &StorageNames, major_version: u32) -> Self {
        let version_dir = format!("v{major_version}");
        let non_empty_env = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            override_file: non_empty_env(names.license_file_env),
            user_dir: user_base(names).join(&version_dir),
            admin_dir: non_empty_env(names.license_dir_env)
                .or_else(|| admin_base(names).map(|d| d.join(&version_dir))),
        }
    }

    pub fn user_license_file(&self) -> PathBuf {
        self.user_dir.join(LICENSE_FILE)
    }

    pub fn activation_file(&self) -> PathBuf {
        self.user_dir.join(ACTIVATION_FILE)
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.user_dir.join(CREDENTIALS_FILE)
    }

    /// Every candidate licence file, in precedence order. Missing files
    /// are skipped silently, except an explicit override file, which is
    /// reported so a misconfigured path is visible.
    pub fn candidates(&self) -> Vec<Candidate> {
        let mut out = Vec::new();
        if let Some(path) = &self.override_file {
            out.push(read_candidate(path));
        }
        let user = self.user_license_file();
        if user.exists() {
            out.push(read_candidate(&user));
        }
        if let Some(dir) = &self.admin_dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut files: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.is_file()
                            && matches!(
                                p.extension().and_then(|e| e.to_str()),
                                Some("license" | "json")
                            )
                    })
                    .collect();
                files.sort();
                out.extend(files.iter().map(|p| read_candidate(p)));
            }
        }
        out
    }

    pub fn read_activation_certificate(&self) -> io::Result<Option<String>> {
        read_optional(&self.activation_file())
    }

    pub fn read_credentials(&self) -> io::Result<Option<ActivationCredentials>> {
        read_optional(&self.credentials_file())?
            .map(|text| {
                serde_json::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            })
            .transpose()
    }

    /// Callers must validate `signed_license` before installing it.
    pub fn write_user_license(&self, signed_license: &str) -> io::Result<()> {
        write_atomic(&self.user_license_file(), signed_license.as_bytes(), false)
    }

    /// Callers must validate `certificate` before installing it.
    pub fn write_activation(
        &self,
        certificate: &str,
        credentials: &ActivationCredentials,
    ) -> io::Result<()> {
        let creds = serde_json::to_vec_pretty(credentials)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&self.credentials_file(), &creds, true)?;
        write_atomic(&self.activation_file(), certificate.as_bytes(), false)
    }

    pub fn remove_activation(&self) -> io::Result<()> {
        remove_if_exists(&self.activation_file())?;
        remove_if_exists(&self.credentials_file())
    }
}

/// A fresh random client secret for a new activation.
pub fn generate_client_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn read_candidate(path: &Path) -> Candidate {
    Candidate {
        source: path.to_path_buf(),
        contents: std::fs::read_to_string(path).map_err(|e| format!("could not read file: {e}")),
    }
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn write_atomic(path: &Path, bytes: &[u8], secret: bool) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        // `mode` only applies when open(2) creates the file; re-assert it
        // in case a stale temporary file survived with looser permissions.
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    // Windows: the per-user directory inherits owner-only ACLs from the
    // user profile; `secret` needs no extra handling there.
    let _ = secret;
    result
}

fn user_base(names: &StorageNames) -> PathBuf {
    let Some(base) = directories::BaseDirs::new() else {
        return PathBuf::from(format!(".{}", names.unix_app));
    };
    if cfg!(windows) {
        base.data_local_dir().join(names.vendor).join(names.app)
    } else if cfg!(target_os = "macos") {
        base.data_dir().join(names.app)
    } else {
        base.config_dir().join(names.unix_app)
    }
}

fn admin_base(names: &StorageNames) -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("ProgramData").map(|d| PathBuf::from(d).join(names.vendor).join(names.app))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Library/Application Support").join(names.app))
    } else {
        Some(PathBuf::from("/etc").join(names.unix_app))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locations(dir: &Path) -> LicenseLocations {
        LicenseLocations {
            override_file: None,
            user_dir: dir.join("user/v2"),
            admin_dir: Some(dir.join("admin")),
        }
    }

    #[test]
    fn no_files_means_no_candidates() {
        let dir = tempfile::tempdir().unwrap();
        assert!(locations(dir.path()).candidates().is_empty());
    }

    #[test]
    fn candidates_are_in_precedence_order() {
        let dir = tempfile::tempdir().unwrap();
        let loc = LicenseLocations {
            override_file: Some(dir.path().join("explicit.license")),
            ..locations(dir.path())
        };
        std::fs::write(dir.path().join("explicit.license"), "e").unwrap();
        loc.write_user_license("u").unwrap();
        std::fs::create_dir_all(dir.path().join("admin")).unwrap();
        std::fs::write(dir.path().join("admin/b.license"), "b").unwrap();
        std::fs::write(dir.path().join("admin/a.json"), "a").unwrap();
        std::fs::write(dir.path().join("admin/readme.txt"), "ignored").unwrap();

        let got: Vec<String> = loc
            .candidates()
            .into_iter()
            .map(|c| c.contents.unwrap())
            .collect();
        assert_eq!(got, vec!["e", "u", "a", "b"]);
    }

    #[test]
    fn a_missing_override_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let loc = LicenseLocations {
            override_file: Some(dir.path().join("missing.license")),
            ..locations(dir.path())
        };
        let c = loc.candidates();
        assert_eq!(c.len(), 1);
        assert!(c[0].contents.is_err());
    }

    #[test]
    fn major_versions_use_separate_user_directories() {
        let names = StorageNames {
            vendor: "V",
            app: "App",
            unix_app: "app",
            license_file_env: "CURSDEL_TEST_UNSET_FILE_ENV",
            license_dir_env: "CURSDEL_TEST_UNSET_DIR_ENV",
        };
        let v2 = LicenseLocations::for_app(&names, 2);
        let v3 = LicenseLocations::for_app(&names, 3);
        assert!(v2.user_dir.ends_with("v2"));
        assert_ne!(v2.user_license_file(), v3.user_license_file());
    }

    #[test]
    fn activation_round_trips_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let creds = ActivationCredentials {
            activation_id: "act_1".into(),
            client_secret: generate_client_secret(),
        };
        loc.write_activation("cert", &creds).unwrap();
        assert_eq!(
            loc.read_activation_certificate().unwrap().as_deref(),
            Some("cert")
        );
        assert_eq!(loc.read_credentials().unwrap(), Some(creds));
        loc.remove_activation().unwrap();
        assert_eq!(loc.read_activation_certificate().unwrap(), None);
        assert_eq!(loc.read_credentials().unwrap(), None);
        loc.remove_activation().unwrap();
    }

    #[test]
    fn rewriting_replaces_contents_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        loc.write_user_license("first").unwrap();
        loc.write_user_license("second").unwrap();
        assert_eq!(
            std::fs::read_to_string(loc.user_license_file()).unwrap(),
            "second"
        );
        let names: Vec<_> = std::fs::read_dir(&loc.user_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("license.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn credentials_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        loc.write_activation(
            "cert",
            &ActivationCredentials {
                activation_id: "a".into(),
                client_secret: "s".into(),
            },
        )
        .unwrap();
        let mode = std::fs::metadata(loc.credentials_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn client_secrets_are_random_32_byte_values() {
        let a = generate_client_secret();
        assert_ne!(a, generate_client_secret());
        assert_eq!(URL_SAFE_NO_PAD.decode(a).unwrap().len(), 32);
    }
}
