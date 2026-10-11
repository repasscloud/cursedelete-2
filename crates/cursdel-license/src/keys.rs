//! Trusted public verification keys.
//!
//! An application build embeds an explicitly chosen set of public keys.
//! Keys are introduced, retained, or removed deliberately per build: a
//! newer major version need not trust an older version's keys, and no
//! build automatically trusts a newly generated key. Private keys never
//! appear in an application.
//!
//! Public keys use each algorithm's standard raw encoding, base64url
//! (no padding) in configuration:
//!
//! - Ed25519: the 32-byte public key (RFC 8032).
//! - ECDSA-P256-SHA256: the SEC1 point, compressed (33 bytes) or
//!   uncompressed (65 bytes).
//! - ML-DSA-65: the 1952-byte encoded verifying key (FIPS 204).

use std::collections::HashSet;
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::algorithm::SignatureAlgorithm;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyError {
    #[error("trusted key '{0}': public key is not valid base64url")]
    Encoding(String),
    #[error("trusted key '{id}': not a valid {algorithm} public key")]
    Invalid {
        id: String,
        algorithm: SignatureAlgorithm,
    },
    #[error("trusted key id '{0}' appears more than once")]
    DuplicateId(String),
}

#[derive(Clone)]
enum PublicKey {
    Ed25519(ed25519_dalek::VerifyingKey),
    EcdsaP256(p256::ecdsa::VerifyingKey),
    MlDsa65(Box<ml_dsa::VerifyingKey<ml_dsa::MlDsa65>>),
}

impl PublicKey {
    fn decode(algorithm: SignatureAlgorithm, bytes: &[u8]) -> Option<Self> {
        match algorithm {
            SignatureAlgorithm::Ed25519 => {
                let raw: [u8; 32] = bytes.try_into().ok()?;
                ed25519_dalek::VerifyingKey::from_bytes(&raw)
                    .ok()
                    .map(PublicKey::Ed25519)
            }
            SignatureAlgorithm::EcdsaP256Sha256 => {
                p256::ecdsa::VerifyingKey::from_sec1_bytes(bytes)
                    .ok()
                    .map(PublicKey::EcdsaP256)
            }
            SignatureAlgorithm::MlDsa65 => {
                let encoded =
                    ml_dsa::EncodedVerifyingKey::<ml_dsa::MlDsa65>::try_from(bytes).ok()?;
                Some(PublicKey::MlDsa65(Box::new(ml_dsa::VerifyingKey::decode(
                    &encoded,
                ))))
            }
        }
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        match self {
            PublicKey::Ed25519(key) => ed25519_dalek::Signature::from_slice(signature)
                .is_ok_and(|sig| key.verify_strict(message, &sig).is_ok()),
            PublicKey::EcdsaP256(key) => {
                use p256::ecdsa::signature::Verifier;
                // Fixed-width IEEE P1363 r||s (64 bytes), not DER.
                p256::ecdsa::Signature::from_slice(signature)
                    .is_ok_and(|sig| key.verify(message, &sig).is_ok())
            }
            PublicKey::MlDsa65(key) => ml_dsa::Signature::<ml_dsa::MlDsa65>::try_from(signature)
                .is_ok_and(|sig| key.verify_with_context(message, &[], &sig)),
        }
    }
}

/// One public key, bound to exactly one algorithm.
#[derive(Clone)]
pub struct TrustedKey {
    id: String,
    algorithm: SignatureAlgorithm,
    key: PublicKey,
}

impl TrustedKey {
    pub fn new(
        id: impl Into<String>,
        algorithm: SignatureAlgorithm,
        public_key: &[u8],
    ) -> Result<Self, KeyError> {
        let id = id.into();
        let key = PublicKey::decode(algorithm, public_key).ok_or_else(|| KeyError::Invalid {
            id: id.clone(),
            algorithm,
        })?;
        Ok(Self { id, algorithm, key })
    }

    pub fn from_base64url(
        id: impl Into<String>,
        algorithm: SignatureAlgorithm,
        public_key: &str,
    ) -> Result<Self, KeyError> {
        let id = id.into();
        let bytes = URL_SAFE_NO_PAD
            .decode(public_key.trim())
            .map_err(|_| KeyError::Encoding(id.clone()))?;
        Self::new(id, algorithm, &bytes)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn algorithm(&self) -> SignatureAlgorithm {
        self.algorithm
    }

    pub(crate) fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        self.key.verify(message, signature)
    }
}

impl fmt::Debug for TrustedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrustedKey")
            .field("id", &self.id)
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}

/// The complete set of keys one application build trusts.
#[derive(Clone, Debug, Default)]
pub struct TrustedKeySet {
    keys: Vec<TrustedKey>,
}

impl TrustedKeySet {
    pub fn new(keys: Vec<TrustedKey>) -> Result<Self, KeyError> {
        let mut seen = HashSet::new();
        for key in &keys {
            if !seen.insert(key.id.as_str()) {
                return Err(KeyError::DuplicateId(key.id.clone()));
            }
        }
        Ok(Self { keys })
    }

    pub fn get(&self, id: &str) -> Option<&TrustedKey> {
        self.keys.iter().find(|k| k.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TrustedKey> {
        self.keys.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestSigner;

    #[test]
    fn every_algorithm_round_trips_through_base64url() {
        for alg in SignatureAlgorithm::ALL {
            let signer = TestSigner::new("k", alg, 7);
            let encoded = URL_SAFE_NO_PAD.encode(signer.public_key_bytes());
            let key = TrustedKey::from_base64url("k", alg, &encoded).unwrap();
            assert_eq!(key.algorithm(), alg);
        }
    }

    #[test]
    fn a_key_is_rejected_under_the_wrong_algorithm() {
        let ed = TestSigner::new("k", SignatureAlgorithm::Ed25519, 7);
        let err = TrustedKey::new("k", SignatureAlgorithm::MlDsa65, &ed.public_key_bytes());
        assert!(matches!(err, Err(KeyError::Invalid { .. })));
    }

    #[test]
    fn duplicate_key_ids_are_rejected() {
        let a = TestSigner::new("same", SignatureAlgorithm::Ed25519, 1).trusted_key();
        let b = TestSigner::new("same", SignatureAlgorithm::EcdsaP256Sha256, 2).trusted_key();
        assert_eq!(
            TrustedKeySet::new(vec![a, b]).unwrap_err(),
            KeyError::DuplicateId("same".into())
        );
    }

    #[test]
    fn invalid_base64_is_reported() {
        assert_eq!(
            TrustedKey::from_base64url("k", SignatureAlgorithm::Ed25519, "!!!").unwrap_err(),
            KeyError::Encoding("k".into())
        );
    }
}
