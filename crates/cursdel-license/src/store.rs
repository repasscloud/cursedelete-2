//! Where licences, Business activation state, and locally known
//! revocations live on disk.
//!
//! There are two writable scopes, each with its own directory per major
//! version (so v2 and v3 installed side by side never touch each other's
//! licences):
//!
//! - [`Scope::User`]: the per-user directory. The default for interactive
//!   use and needs no privileges.
//! - [`Scope::System`]: the machine-wide, administrator-managed directory.
//!   Used by installers and site-wide deployment. Administrators may also
//!   drop licence files in here by hand.
//!
//! | Platform | User directory | System directory |
//! |---|---|---|
//! | Windows | `%LOCALAPPDATA%\<Vendor>\<App>\v<N>` | `%ProgramData%\<Vendor>\<App>\v<N>` |
//! | macOS | `~/Library/Application Support/<App>/v<N>` | `/Library/Application Support/<App>/v<N>` |
//! | Linux | `$XDG_CONFIG_HOME/<app>/v<N>` | `/etc/<app>/v<N>` |
//!
//! The system directory can be replaced with the application's
//! `*_LICENSE_DIR` environment variable. Candidate licences are read, in
//! precedence order, from: an explicit override file (`*_LICENSE_FILE` or
//! a CLI flag), the user `license.json`, then every `*.license`/`*.json`
//! file in the system directory except the reserved state files below.
//!
//! Each scope directory may also hold `activation.json` (a signed Business
//! activation certificate), `credentials.json` (its client secret), and
//! `revocations.json` (licence ids known to be revoked). The client secret
//! is owner-only (`0600`) on Unix; on Windows a system-scope secret is
//! restricted to SYSTEM and Administrators, while the user directory
//! already inherits owner-only ACLs from the profile. Every write goes to a
//! temporary file and is renamed into place, so an interrupted write never
//! leaves a half-written file.

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
    /// Environment variable replacing the system directory.
    pub license_dir_env: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseLocations {
    pub override_file: Option<PathBuf>,
    pub user_dir: PathBuf,
    pub system_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationCredentials {
    pub activation_id: String,
    /// Bearer secret proving this installation owns the activation.
    /// Generated locally, sent only to the licensing server, never printed.
    pub client_secret: String,
}

/// A Business activation certificate found on disk, with its scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredActivation {
    pub scope: Scope,
    pub certificate: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Revocations {
    revoked_license_ids: Vec<String>,
}

const LICENSE_FILE: &str = "license.json";
const ACTIVATION_FILE: &str = "activation.json";
const CREDENTIALS_FILE: &str = "credentials.json";
const REVOCATIONS_FILE: &str = "revocations.json";
/// State files that share the system directory with licence files and
/// must never be read as candidate licences.
const RESERVED: &[&str] = &[ACTIVATION_FILE, CREDENTIALS_FILE, REVOCATIONS_FILE];

#[derive(Clone, Copy)]
enum Secrecy {
    Public,
    Secret(Scope),
}

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
            system_dir: non_empty_env(names.license_dir_env)
                .or_else(|| system_base(names).map(|d| d.join(&version_dir))),
        }
    }

    pub fn dir(&self, scope: Scope) -> io::Result<&Path> {
        match scope {
            Scope::User => Ok(&self.user_dir),
            Scope::System => self.system_dir.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "no system licence directory is available on this platform",
                )
            }),
        }
    }

    fn file(&self, scope: Scope, name: &str) -> io::Result<PathBuf> {
        Ok(self.dir(scope)?.join(name))
    }

    /// Where `install`/`activate`/`renew` write a licence in `scope`.
    pub fn license_file(&self, scope: Scope) -> io::Result<PathBuf> {
        self.file(scope, LICENSE_FILE)
    }

    pub fn credentials_file(&self, scope: Scope) -> io::Result<PathBuf> {
        self.file(scope, CREDENTIALS_FILE)
    }

    /// Every candidate licence file, in precedence order. Missing files
    /// are skipped silently, except an explicit override file, which is
    /// reported so a misconfigured path is visible.
    pub fn candidates(&self) -> Vec<Candidate> {
        let mut out = Vec::new();
        if let Some(path) = &self.override_file {
            out.push(read_candidate(path));
        }
        out.extend(self.candidates_in(Scope::User));
        out.extend(self.candidates_in(Scope::System));
        out
    }

    /// Candidate licences stored in one scope (no override file).
    pub fn candidates_in(&self, scope: Scope) -> Vec<Candidate> {
        let Ok(dir) = self.dir(scope) else {
            return Vec::new();
        };
        if scope == Scope::User {
            let path = dir.join(LICENSE_FILE);
            return if path.exists() {
                vec![read_candidate(&path)]
            } else {
                Vec::new()
            };
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                p.is_file()
                    && !RESERVED.contains(&name)
                    && !name.starts_with('.')
                    && matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("license" | "json")
                    )
            })
            .collect();
        files.sort();
        files.iter().map(|p| read_candidate(p)).collect()
    }

    /// Activation certificates from both scopes, user first. Unreadable
    /// files are skipped: a missing certificate only means "not activated".
    pub fn activation_certificates(&self) -> Vec<StoredActivation> {
        [Scope::User, Scope::System]
            .into_iter()
            .filter_map(|scope| {
                let path = self.file(scope, ACTIVATION_FILE).ok()?;
                let certificate = read_optional(&path).ok()??;
                Some(StoredActivation { scope, certificate })
            })
            .collect()
    }

    pub fn read_credentials(&self, scope: Scope) -> io::Result<Option<ActivationCredentials>> {
        read_optional(&self.file(scope, CREDENTIALS_FILE)?)?
            .map(|text| {
                serde_json::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            })
            .transpose()
    }

    /// Callers must validate `signed_license` before installing it.
    pub fn write_license(&self, scope: Scope, signed_license: &str) -> io::Result<()> {
        write_atomic(
            &self.license_file(scope)?,
            signed_license.as_bytes(),
            Secrecy::Public,
        )
    }

    /// Callers must validate `certificate` before installing it.
    pub fn write_activation(
        &self,
        scope: Scope,
        certificate: &str,
        credentials: &ActivationCredentials,
    ) -> io::Result<()> {
        let creds = serde_json::to_vec_pretty(credentials)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(
            &self.file(scope, CREDENTIALS_FILE)?,
            &creds,
            Secrecy::Secret(scope),
        )?;
        write_atomic(
            &self.file(scope, ACTIVATION_FILE)?,
            certificate.as_bytes(),
            Secrecy::Public,
        )
    }

    pub fn remove_activation(&self, scope: Scope) -> io::Result<()> {
        remove_if_exists(&self.file(scope, ACTIVATION_FILE)?)?;
        remove_if_exists(&self.file(scope, CREDENTIALS_FILE)?)
    }

    /// Licence ids recorded as revoked in either scope -- by this
    /// application after the server reported a revocation, or by an
    /// administrator (`{"revokedLicenseIds": [...]}` in the system
    /// directory). An unsigned list is safe to honour because it can only
    /// take entitlement away. Unreadable or malformed files are ignored.
    pub fn revoked_license_ids(&self) -> Vec<String> {
        [Scope::User, Scope::System]
            .into_iter()
            .filter_map(|scope| self.read_revocations(scope).ok())
            .flat_map(|r| r.revoked_license_ids)
            .collect()
    }

    pub fn record_revocation(&self, scope: Scope, license_id: &str) -> io::Result<()> {
        let mut r = self.read_revocations(scope).unwrap_or_default();
        if !r.revoked_license_ids.iter().any(|id| id == license_id) {
            r.revoked_license_ids.push(license_id.to_string());
            self.write_revocations(scope, &r)?;
        }
        Ok(())
    }

    /// Undo a recorded revocation after the server issued a fresh licence.
    pub fn clear_revocation(&self, scope: Scope, license_id: &str) -> io::Result<()> {
        let Ok(mut r) = self.read_revocations(scope) else {
            return Ok(());
        };
        let before = r.revoked_license_ids.len();
        r.revoked_license_ids.retain(|id| id != license_id);
        if r.revoked_license_ids.len() != before {
            self.write_revocations(scope, &r)?;
        }
        Ok(())
    }

    fn read_revocations(&self, scope: Scope) -> io::Result<Revocations> {
        match read_optional(&self.file(scope, REVOCATIONS_FILE)?)? {
            Some(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            None => Ok(Revocations::default()),
        }
    }

    fn write_revocations(&self, scope: Scope, r: &Revocations) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(r)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(
            &self.file(scope, REVOCATIONS_FILE)?,
            &bytes,
            Secrecy::Public,
        )
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

fn write_atomic(path: &Path, bytes: &[u8], secrecy: Secrecy) -> io::Result<()> {
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
    #[cfg(unix)]
    let secret = matches!(secrecy, Secrecy::Secret(_));

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
        // Restrict before any secret bytes are written.
        if let Secrecy::Secret(Scope::System) = secrecy {
            restrict_to_administrators(&tmp)?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Windows: replace the inherited ACL (ProgramData grants Users read) with
/// full control for SYSTEM (S-1-5-18) and Administrators (S-1-5-32-544)
/// only. Well-known SIDs avoid localised group names. Elsewhere `0600`
/// already limits the file to its (root) owner.
#[cfg(windows)]
fn restrict_to_administrators(path: &Path) -> io::Result<()> {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args([
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-18:F",
            "*S-1-5-32-544:F",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not restrict the credentials file to administrators (icacls failed)",
        ))
    }
}

#[cfg(not(windows))]
fn restrict_to_administrators(_path: &Path) -> io::Result<()> {
    Ok(())
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

fn system_base(names: &StorageNames) -> Option<PathBuf> {
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
            system_dir: Some(dir.join("system")),
        }
    }

    fn creds(id: &str) -> ActivationCredentials {
        ActivationCredentials {
            activation_id: id.into(),
            client_secret: generate_client_secret(),
        }
    }

    #[test]
    fn no_files_means_no_candidates() {
        let dir = tempfile::tempdir().unwrap();
        assert!(locations(dir.path()).candidates().is_empty());
    }

    #[test]
    fn candidates_are_in_precedence_order_and_skip_state_files() {
        let dir = tempfile::tempdir().unwrap();
        let loc = LicenseLocations {
            override_file: Some(dir.path().join("explicit.license")),
            ..locations(dir.path())
        };
        std::fs::write(dir.path().join("explicit.license"), "e").unwrap();
        loc.write_license(Scope::User, "u").unwrap();
        loc.write_license(Scope::System, "s").unwrap();
        loc.write_activation(Scope::System, "cert", &creds("a"))
            .unwrap();
        loc.record_revocation(Scope::System, "lic_x").unwrap();
        let sys = dir.path().join("system");
        std::fs::write(sys.join("b.license"), "b").unwrap();
        std::fs::write(sys.join("a.json"), "a").unwrap();
        std::fs::write(sys.join("readme.txt"), "ignored").unwrap();

        let got: Vec<String> = loc
            .candidates()
            .into_iter()
            .map(|c| c.contents.unwrap())
            .collect();
        assert_eq!(got, vec!["e", "u", "a", "b", "s"]);
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
    fn major_versions_use_separate_directories() {
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
        assert_ne!(
            v2.license_file(Scope::User).unwrap(),
            v3.license_file(Scope::User).unwrap()
        );
        if let (Some(a), Some(b)) = (&v2.system_dir, &v3.system_dir) {
            assert_ne!(a, b);
        }
    }

    #[test]
    fn activation_round_trips_per_scope_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let user = creds("act_user");
        loc.write_activation(Scope::User, "cert-u", &user).unwrap();
        loc.write_activation(Scope::System, "cert-s", &creds("act_sys"))
            .unwrap();
        assert_eq!(
            loc.activation_certificates(),
            vec![
                StoredActivation {
                    scope: Scope::User,
                    certificate: "cert-u".into()
                },
                StoredActivation {
                    scope: Scope::System,
                    certificate: "cert-s".into()
                },
            ]
        );
        assert_eq!(loc.read_credentials(Scope::User).unwrap(), Some(user));
        loc.remove_activation(Scope::User).unwrap();
        assert_eq!(loc.read_credentials(Scope::User).unwrap(), None);
        assert_eq!(loc.activation_certificates().len(), 1);
        loc.remove_activation(Scope::User).unwrap();
    }

    #[test]
    fn revocations_are_recorded_merged_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        loc.record_revocation(Scope::User, "lic_a").unwrap();
        loc.record_revocation(Scope::User, "lic_a").unwrap();
        loc.record_revocation(Scope::System, "lic_b").unwrap();
        assert_eq!(loc.revoked_license_ids(), vec!["lic_a", "lic_b"]);
        loc.clear_revocation(Scope::User, "lic_a").unwrap();
        assert_eq!(loc.revoked_license_ids(), vec!["lic_b"]);
    }

    #[test]
    fn administrator_revocation_list_format_is_honoured_and_malformed_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        std::fs::create_dir_all(dir.path().join("system")).unwrap();
        std::fs::write(
            dir.path().join("system/revocations.json"),
            r#"{"revokedLicenseIds": ["lic_admin"]}"#,
        )
        .unwrap();
        assert_eq!(loc.revoked_license_ids(), vec!["lic_admin"]);
        std::fs::write(dir.path().join("system/revocations.json"), "garbage").unwrap();
        assert!(loc.revoked_license_ids().is_empty());
    }

    #[test]
    fn no_system_directory_is_an_error_not_a_panic() {
        let loc = LicenseLocations {
            override_file: None,
            user_dir: PathBuf::from("u"),
            system_dir: None,
        };
        assert!(loc.write_license(Scope::System, "x").is_err());
        assert!(loc.candidates_in(Scope::System).is_empty());
    }

    #[test]
    fn rewriting_replaces_contents_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        loc.write_license(Scope::User, "first").unwrap();
        loc.write_license(Scope::User, "second").unwrap();
        let path = loc.license_file(Scope::User).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let names: Vec<_> = std::fs::read_dir(&loc.user_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("license.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn credentials_are_owner_only_in_both_scopes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        for scope in [Scope::User, Scope::System] {
            loc.write_activation(scope, "cert", &creds("a")).unwrap();
            let mode = std::fs::metadata(loc.credentials_file(scope).unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn client_secrets_are_random_32_byte_values() {
        let a = generate_client_secret();
        assert_ne!(a, generate_client_secret());
        assert_eq!(URL_SAFE_NO_PAD.decode(a).unwrap().len(), 32);
    }
}
