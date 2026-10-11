//! The signature algorithms a signed licence envelope may use.
//!
//! Every signing keypair is bound to exactly one algorithm (see
//! [`crate::keys::TrustedKey`]), and every application additionally keeps
//! an explicit allowlist of the algorithms it is willing to accept (see
//! [`crate::validate::LicensePolicy::allowed_algorithms`]). Supporting an
//! algorithm here only means this library *can* verify it.
//!
//! - **Ed25519** (RFC 8032): the default for ordinary software licences.
//! - **ECDSA-P256-SHA256** (FIPS 186-5): for deployments that require a
//!   FIPS-approved algorithm. Choosing it does not by itself make an
//!   application FIPS-compliant -- that also depends on a validated
//!   cryptographic module and operating environment, which this pure-Rust
//!   implementation is not.
//! - **ML-DSA-65** (FIPS 204): quantum-resistant signatures for long-term
//!   resilience. Licences can be dual-signed (e.g. Ed25519 + ML-DSA-65)
//!   while application builds migrate.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignatureAlgorithm {
    Ed25519,
    EcdsaP256Sha256,
    MlDsa65,
}

impl SignatureAlgorithm {
    pub const ALL: [SignatureAlgorithm; 3] = [
        SignatureAlgorithm::Ed25519,
        SignatureAlgorithm::EcdsaP256Sha256,
        SignatureAlgorithm::MlDsa65,
    ];

    /// The wire identifier used in an envelope's `signatureAlgorithm`.
    pub fn id(self) -> &'static str {
        match self {
            SignatureAlgorithm::Ed25519 => "Ed25519",
            SignatureAlgorithm::EcdsaP256Sha256 => "ECDSA-P256-SHA256",
            SignatureAlgorithm::MlDsa65 => "ML-DSA-65",
        }
    }

    /// Exact, case-sensitive match on the wire identifier. Anything else
    /// is an algorithm this build does not support.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

impl fmt::Display for SignatureAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for alg in SignatureAlgorithm::ALL {
            assert_eq!(SignatureAlgorithm::from_id(alg.id()), Some(alg));
        }
    }

    #[test]
    fn ids_are_case_sensitive() {
        assert_eq!(SignatureAlgorithm::from_id("ed25519"), None);
        assert_eq!(SignatureAlgorithm::from_id("ML-DSA-87"), None);
    }
}
