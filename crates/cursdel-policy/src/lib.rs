//! CurseDelete's application-specific licensing configuration -- the one
//! place that decides product identity, which signing keys and algorithms
//! this build trusts, where licences live, which licensing server to use,
//! and which features each edition enables. `cursdel-license` supplies
//! the product-agnostic mechanics; `cursdel-cli` consumes the result.
//!
//! The deletion engine (`cursdel-core`) never consumes this crate: raw
//! deletion speed and the core capabilities (adaptive/manual workers,
//! `--force` remediation, retention, filters, dry-run, JSON output,
//! `--kill-locks`) are identical in every edition. Commercial-use rights,
//! deployment scale, and support are licensing terms, not runtime checks.

use cursdel_license::http::{HttpLicenseService, InsecureUrl};
use cursdel_license::service::UnconfiguredService;
use cursdel_license::{
    Edition, LicenseLocations, LicensePolicy, LicenseService, ProductIdentity, SignatureAlgorithm,
    StorageNames, TrustedKey, TrustedKeySet,
};

pub const PRODUCT_ID: &str = "cursedelete";

/// The licensed major version is this build's own Cargo major version.
pub fn major_version() -> u32 {
    env!("CARGO_PKG_VERSION_MAJOR")
        .parse()
        .expect("Cargo major version is numeric")
}

/// Every algorithm the library supports is permitted. Narrow this (e.g.
/// to ECDSA-P256-SHA256 only) for a build that must accept FIPS-approved
/// algorithms exclusively.
pub const ALLOWED_ALGORITHMS: &[SignatureAlgorithm] = &[
    SignatureAlgorithm::Ed25519,
    SignatureAlgorithm::EcdsaP256Sha256,
    SignatureAlgorithm::MlDsa65,
];

/// Public keys this v2 build trusts: `(keyId, algorithm, base64url public
/// key)`. Add a key here only once the licensing server has published it
/// and before it is used to issue CurseDelete v2 licences; keep a key for
/// as long as licences it signed must keep working.
///
/// Intentionally empty: no production signing keys exist yet, and they
/// are never generated in this repository. Until one is added, every
/// licence is rejected as signed by an untrusted key and the application
/// runs as Community -- fully functional apart from the gated features.
const TRUSTED_KEYS: &[(&str, SignatureAlgorithm, &str)] = &[];

pub const STORAGE: StorageNames = StorageNames {
    vendor: "RePassCloud",
    app: "CurseDelete",
    unix_app: "cursedelete",
    license_file_env: "CURSDEL_LICENSE_FILE",
    license_dir_env: "CURSDEL_LICENSE_DIR",
};

/// Namespace for the hashed Business device id (see
/// `cursdel_license::device`). Changing it changes every device id.
pub const DEVICE_NAMESPACE: &str = "CurseDelete.DeviceIdentity.v1";

/// Runtime override for the licensing server URL. A default can be baked
/// in at build time by setting the same variable when compiling.
pub const SERVER_URL_ENV: &str = "CURSDEL_LICENSE_SERVER_URL";

pub fn trusted_keys() -> TrustedKeySet {
    let keys = TRUSTED_KEYS
        .iter()
        .map(|(id, alg, key)| {
            TrustedKey::from_base64url(*id, *alg, key).expect("embedded public key is valid")
        })
        .collect();
    TrustedKeySet::new(keys).expect("embedded key ids are unique")
}

pub fn product() -> ProductIdentity {
    ProductIdentity {
        product_id: PRODUCT_ID.to_string(),
        major_version: major_version(),
    }
}

/// CurseDelete activates Business online once and then works offline
/// indefinitely: activation certificates carry their own expiry if the
/// server sets one, but there is no local refresh deadline. Portal
/// deactivation therefore takes effect on this device only at its next
/// successful `license renew` (see `docs/LICENSING.md`).
pub fn license_policy() -> LicensePolicy {
    LicensePolicy {
        product: product(),
        trusted_keys: trusted_keys(),
        allowed_algorithms: ALLOWED_ALGORITHMS.to_vec(),
        business_offline_limit: None,
    }
}

pub fn locations() -> LicenseLocations {
    LicenseLocations::for_app(&STORAGE, major_version())
}

/// The licensing server client: the runtime `CURSDEL_LICENSE_SERVER_URL`,
/// else a URL baked in at build time, else none (offline licensing still
/// works; online commands report that no server is configured).
pub fn service() -> Result<Box<dyn LicenseService>, InsecureUrl> {
    let url = std::env::var(SERVER_URL_ENV)
        .ok()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| option_env!("CURSDEL_LICENSE_SERVER_URL").map(str::to_string));
    Ok(match url {
        Some(url) => Box::new(HttpLicenseService::new(&url)?),
        None => Box::new(UnconfiguredService),
    })
}

/// Features whose availability depends on the edition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// `--close-remote-locks`: administratively closing remote SMB opens
    /// on a Windows file server.
    CloseRemoteLocks,
}

impl Feature {
    pub const ALL: [Feature; 1] = [Feature::CloseRemoteLocks];

    pub fn name(self) -> &'static str {
        match self {
            Feature::CloseRemoteLocks => "--close-remote-locks",
        }
    }

    /// The edition feature table. Registered Community currently unlocks
    /// nothing beyond unregistered Community.
    pub fn is_enabled(self, edition: Edition) -> bool {
        match self {
            Feature::CloseRemoteLocks => matches!(edition, Edition::Business | Edition::Enterprise),
        }
    }

    pub fn editions(self) -> Vec<Edition> {
        Edition::ALL
            .into_iter()
            .filter(|e| self.is_enabled(*e))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_version_matches_the_crate_version() {
        assert_eq!(
            major_version().to_string(),
            env!("CARGO_PKG_VERSION").split('.').next().unwrap()
        );
    }

    #[test]
    fn embedded_trusted_keys_are_valid_and_unique() {
        // Panics if any embedded key is malformed or duplicated.
        let keys = trusted_keys();
        for key in keys.iter() {
            assert!(ALLOWED_ALGORITHMS.contains(&key.algorithm()));
        }
    }

    #[test]
    fn close_remote_locks_is_business_and_enterprise_only() {
        assert_eq!(
            Feature::CloseRemoteLocks.editions(),
            vec![Edition::Business, Edition::Enterprise]
        );
    }

    #[test]
    fn every_feature_is_enabled_for_enterprise() {
        for f in Feature::ALL {
            assert!(f.is_enabled(Edition::Enterprise), "{}", f.name());
        }
    }

    #[test]
    fn community_has_no_gated_features() {
        for f in Feature::ALL {
            assert!(!f.is_enabled(Edition::Community), "{}", f.name());
        }
    }
}
