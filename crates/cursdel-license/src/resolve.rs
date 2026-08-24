//! Machine -> user -> Community licence resolution.
//!
//! CurseDelete supports two licence storage scopes: machine-wide (written
//! by `cursdel license enroll`, a Deployment Key) and per-user (written by
//! `cursdel license activate`/`import`, a manual License ID + Activation
//! Code). A machine can have state in either, both, or neither. This
//! module is the single place that decides which one, if any, is
//! currently active.
//!
//! The load-bearing rule (see the machine-wide storage/resolution
//! product requirement): an invalid machine-wide licence is **not**
//! silently bypassed in favour of a valid user licence. If machine-wide
//! state exists but fails to verify or doesn't cover this product, that
//! is reported as [`Resolution::Invalid`] -- the caller decides how to
//! present that (`cursdel license status` shows it; ordinary deletion
//! runs fail open to Community, per this crate's existing "an invalid
//! licence must never crash cursdel" policy, but it does so having
//! *seen* the invalid machine licence, not by silently preferring
//! whatever older per-user licence happens to still be on disk).

use crate::error::ValidationError;
use crate::schema::ProductEntitlement;
use crate::store::{self, LicensePaths};
use crate::verify::{self, VerifiedLicense};

/// Which storage scope a resolved (or invalid) licence was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseScope {
    /// Written by `cursdel license enroll` (Deployment Key). Shared by
    /// every account on the machine.
    Machine,
    /// Written by `cursdel license activate`/`import`. Specific to the
    /// current user account.
    User,
}

impl LicenseScope {
    pub fn label(self) -> &'static str {
        match self {
            LicenseScope::Machine => "machine",
            LicenseScope::User => "user",
        }
    }
}

/// A licence that resolved successfully: verified, and covering the
/// requested product.
#[derive(Debug, Clone)]
pub struct ResolvedLicense {
    pub scope: LicenseScope,
    pub paths: LicensePaths,
    pub verified: VerifiedLicense,
    pub entitlement: ProductEntitlement,
}

/// The outcome of [`resolve_active_license`].
pub enum Resolution {
    /// A verified, currently-valid licence for the requested product.
    /// Boxed: `ResolvedLicense` embeds the full parsed licence payload,
    /// making it far larger than this enum's other variants.
    Active(Box<ResolvedLicense>),
    /// A licence file exists at `scope` but is unreadable, fails
    /// signature/schema verification, doesn't cover the requested
    /// product, or is expired. Resolution stops here -- a lower-priority
    /// scope is deliberately **not** consulted, so a broken machine-wide
    /// licence can never be silently rescued by a stale per-user one.
    Invalid {
        scope: LicenseScope,
        paths: LicensePaths,
        error: ValidationError,
    },
    /// No licence file at either scope.
    Community,
}

/// Resolves the active licence for `product`, preferring machine-wide
/// state over per-user state (see the module documentation for the
/// fail-closed rule governing an invalid higher-priority licence).
pub fn resolve_active_license(product: &str) -> Resolution {
    resolve_from_scopes(
        product,
        [
            (LicenseScope::Machine, store::machine_wide_paths()),
            (LicenseScope::User, store::default_paths()),
        ],
    )
}

/// The actual resolution logic, parameterised over explicit
/// `(scope, paths)` pairs in priority order so it's testable against
/// throwaway temp-directory paths without touching real machine-wide
/// state or per-process-global environment overrides.
pub(crate) fn resolve_from_scopes(
    product: &str,
    scopes: impl IntoIterator<Item = (LicenseScope, LicensePaths)>,
) -> Resolution {
    for (scope, paths) in scopes {
        match try_scope(&paths, product) {
            ScopeOutcome::Absent => continue,
            ScopeOutcome::Invalid(error) => {
                return Resolution::Invalid {
                    scope,
                    paths,
                    error,
                }
            }
            ScopeOutcome::Active(verified, entitlement) => {
                return Resolution::Active(Box::new(ResolvedLicense {
                    scope,
                    paths,
                    verified: *verified,
                    entitlement,
                }))
            }
        }
    }
    Resolution::Community
}

enum ScopeOutcome {
    /// No licence file at this scope at all -- not an error, just "try
    /// the next scope".
    Absent,
    /// A licence file exists at this scope but isn't currently usable.
    Invalid(ValidationError),
    Active(Box<VerifiedLicense>, ProductEntitlement),
}

fn try_scope(paths: &LicensePaths, product: &str) -> ScopeOutcome {
    let license_json = match store::load_license_file(paths) {
        Ok(Some(text)) => text,
        Ok(None) => return ScopeOutcome::Absent,
        // A file that exists but can't even be read (permissions,
        // I/O error) is "present but broken", not "absent" -- it must
        // not be silently skipped in favour of a lower-priority scope.
        Err(e) => {
            return ScopeOutcome::Invalid(ValidationError::new(format!(
                "could not read licence file at {}: {e}",
                paths.license_file.display()
            )))
        }
    };

    let verified = match verify::verify(&license_json) {
        Ok(v) => v,
        Err(e) => return ScopeOutcome::Invalid(e),
    };

    match verify::validate_product(&verified, product, None, None, None) {
        Ok(entitlement) => ScopeOutcome::Active(Box::new(verified), entitlement),
        Err(e) => ScopeOutcome::Invalid(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// A signed licence that verifies against this crate's compiled-in
    /// trust store would require production key material this repo
    /// deliberately does not have (see `tests/fixtures/README.md`), so
    /// these tests exercise `resolve_from_scopes`'s *precedence and
    /// fail-closed* logic -- which does not depend on whether a licence
    /// is trusted, only on whether a file is present and what
    /// `try_scope` decides about it -- using the untrusted-key test
    /// fixture (already used the same way by the CLI's own
    /// `license_import_rejects_a_license_signed_by_an_untrusted_key`
    /// test) as a stand-in for "a licence file is present but invalid".
    fn write_untrusted_license(dir: &std::path::Path) -> LicensePaths {
        std::fs::create_dir_all(dir).unwrap();
        let license_file = dir.join("license.json");
        let activation_credentials_file = dir.join("activation.json");
        std::fs::write(&license_file, fixture("test.license")).unwrap();
        LicensePaths {
            license_file,
            activation_credentials_file,
        }
    }

    fn empty_paths(dir: &std::path::Path) -> LicensePaths {
        LicensePaths {
            license_file: dir.join("license.json"),
            activation_credentials_file: dir.join("activation.json"),
        }
    }

    #[test]
    fn neither_scope_present_resolves_to_community() {
        let machine_dir = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();
        let resolution = resolve_from_scopes(
            "cursedelete",
            [
                (LicenseScope::Machine, empty_paths(machine_dir.path())),
                (LicenseScope::User, empty_paths(user_dir.path())),
            ],
        );
        assert!(matches!(resolution, Resolution::Community));
    }

    #[test]
    fn machine_absent_user_present_but_invalid_reports_user_scope() {
        // Neither scope has a *trusted*-signed licence available in this
        // repo (see the fixture note above), so "user present" here means
        // "a licence file exists at the user scope, and resolution
        // reports it as belonging to User scope" -- proving fallthrough
        // to the next scope happens when the higher-priority scope is
        // genuinely absent (not merely invalid).
        let machine_dir = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();
        let user_paths = write_untrusted_license(user_dir.path());

        let resolution = resolve_from_scopes(
            "cursedelete",
            [
                (LicenseScope::Machine, empty_paths(machine_dir.path())),
                (LicenseScope::User, user_paths),
            ],
        );

        match resolution {
            Resolution::Invalid { scope, .. } => assert_eq!(scope, LicenseScope::User),
            _ => panic!("expected an Invalid resolution at User scope"),
        }
    }

    #[test]
    fn invalid_machine_licence_is_not_bypassed_by_a_valid_user_licence() {
        // The core fail-closed requirement: a present-but-invalid
        // machine-wide licence must stop resolution at Machine scope,
        // never silently falling through to check User scope at all --
        // even though, in this test, a licence file also exists there.
        let machine_dir = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();
        let machine_paths = write_untrusted_license(machine_dir.path());
        let user_paths = write_untrusted_license(user_dir.path());

        let resolution = resolve_from_scopes(
            "cursedelete",
            [
                (LicenseScope::Machine, machine_paths),
                (LicenseScope::User, user_paths),
            ],
        );

        match resolution {
            Resolution::Invalid { scope, .. } => assert_eq!(scope, LicenseScope::Machine),
            _ => panic!("expected an Invalid resolution at Machine scope, not a fallthrough"),
        }
    }

    #[test]
    fn unreadable_license_file_is_invalid_not_absent() {
        // A scope whose license.json exists but can't be read (here:
        // it's a directory, not a file, forcing a real I/O error rather
        // than a NotFound) must be treated as "present but broken", not
        // silently skipped as if nothing were configured there.
        let dir = tempfile::tempdir().unwrap();
        let paths = empty_paths(dir.path());
        std::fs::create_dir_all(&paths.license_file).unwrap();

        let user_dir = tempfile::tempdir().unwrap();
        let resolution = resolve_from_scopes(
            "cursedelete",
            [
                (LicenseScope::Machine, paths),
                (LicenseScope::User, empty_paths(user_dir.path())),
            ],
        );

        match resolution {
            Resolution::Invalid { scope, .. } => assert_eq!(scope, LicenseScope::Machine),
            _ => panic!("expected an unreadable licence file to resolve as Invalid"),
        }
    }
}
