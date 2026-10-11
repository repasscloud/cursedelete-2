//! Product-agnostic application-side licensing.
//!
//! - [`envelope`]: the signed envelope (Ed25519, ECDSA-P256-SHA256,
//!   ML-DSA-65; single or multiple signatures).
//! - [`keys`]: an application build's explicitly chosen trusted keys.
//! - [`claims`]: licence and Business activation-certificate claims.
//! - [`validate`]: offline validation for one product and major version.
//! - [`resolve`]: picking the effective licence from installed files.
//! - [`store`]: per-platform, per-major-version licence locations.
//! - [`service`] / [`http`]: the boundary to the licensing server.
//!
//! Nothing here knows about any particular application or which features
//! an edition unlocks; each application supplies its own
//! [`LicensePolicy`] and feature mapping. See
//! `docs/LICENSING-ARCHITECTURE.md` for the architecture this implements.

pub mod algorithm;
pub mod claims;
pub mod device;
pub mod edition;
pub mod envelope;
pub mod http;
pub mod keys;
pub mod resolve;
pub mod service;
pub mod store;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod validate;

pub use algorithm::SignatureAlgorithm;
pub use claims::{ActivationClaims, LicenseClaims, Term};
pub use edition::Edition;
pub use keys::{TrustedKey, TrustedKeySet};
pub use resolve::{resolve, ActiveLicense, Resolution};
pub use service::{LicenseService, ServiceError};
pub use store::{LicenseLocations, StorageNames};
pub use validate::{LicensePolicy, ProductIdentity, Rejection, VerifiedLicense};
