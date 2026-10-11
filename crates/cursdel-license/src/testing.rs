//! Test-only signing helpers. Compiled for this crate's own tests and
//! for downstream crates that enable the `test-support` feature from
//! `[dev-dependencies]`.
//!
//! Keys here are derived from a one-byte seed, so they are deterministic
//! and **publicly known**. They exist only to produce signed fixtures in
//! tests and must never be trusted by a shipped build.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};

use crate::algorithm::SignatureAlgorithm;
use crate::edition::Edition;
use crate::envelope::{signed_message, EnvelopeKind, RawEnvelope, RawSignature};
use crate::keys::TrustedKey;

enum SigningKey {
    Ed25519(ed25519_dalek::SigningKey),
    EcdsaP256(p256::ecdsa::SigningKey),
    MlDsa65(Box<ml_dsa::SigningKey<ml_dsa::MlDsa65>>),
}

pub struct TestSigner {
    key_id: String,
    algorithm: SignatureAlgorithm,
    key: SigningKey,
}

impl TestSigner {
    /// `seed` must be non-zero.
    pub fn new(key_id: &str, algorithm: SignatureAlgorithm, seed: u8) -> Self {
        assert_ne!(seed, 0, "seed must be non-zero");
        let bytes = [seed; 32];
        let key = match algorithm {
            SignatureAlgorithm::Ed25519 => {
                SigningKey::Ed25519(ed25519_dalek::SigningKey::from_bytes(&bytes))
            }
            SignatureAlgorithm::EcdsaP256Sha256 => SigningKey::EcdsaP256(
                p256::ecdsa::SigningKey::from_slice(&bytes).expect("valid P-256 scalar"),
            ),
            SignatureAlgorithm::MlDsa65 => {
                SigningKey::MlDsa65(Box::new(ml_dsa::SigningKey::<ml_dsa::MlDsa65>::from_seed(
                    &bytes.into(),
                )))
            }
        };
        Self {
            key_id: key_id.to_string(),
            algorithm,
            key,
        }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        match &self.key {
            SigningKey::Ed25519(k) => k.verifying_key().to_bytes().to_vec(),
            SigningKey::EcdsaP256(k) => {
                k.verifying_key().to_encoded_point(true).as_bytes().to_vec()
            }
            SigningKey::MlDsa65(k) => {
                use ml_dsa::Keypair;
                k.verifying_key().encode().to_vec()
            }
        }
    }

    pub fn trusted_key(&self) -> TrustedKey {
        TrustedKey::new(&self.key_id, self.algorithm, &self.public_key_bytes())
            .expect("test key is valid")
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        match &self.key {
            SigningKey::Ed25519(k) => {
                use ed25519_dalek::Signer;
                k.sign(message).to_bytes().to_vec()
            }
            SigningKey::EcdsaP256(k) => {
                use p256::ecdsa::signature::Signer;
                let sig: p256::ecdsa::Signature = k.sign(message);
                sig.to_bytes().to_vec()
            }
            SigningKey::MlDsa65(k) => k
                .expanded_key()
                .sign_deterministic(message, &[])
                .expect("empty context is valid")
                .encode()
                .to_vec(),
        }
    }
}

/// Seal raw payload bytes into an envelope signed by every `signer`.
pub fn seal_bytes(kind: EnvelopeKind, payload: &[u8], signers: &[&TestSigner]) -> String {
    let message = signed_message(kind, payload);
    let envelope = RawEnvelope {
        format: kind.format().to_string(),
        payload: URL_SAFE_NO_PAD.encode(payload),
        signatures: signers
            .iter()
            .map(|s| RawSignature {
                signature_algorithm: s.algorithm.id().to_string(),
                key_id: s.key_id.clone(),
                signature: URL_SAFE_NO_PAD.encode(s.sign(&message)),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&envelope).expect("envelope serialises")
}

pub fn seal(kind: EnvelopeKind, claims: &Value, signers: &[&TestSigner]) -> String {
    seal_bytes(kind, claims.to_string().as_bytes(), signers)
}

/// Valid default licence claims for `edition`: perpetual, issued
/// 2026-01-01, with a 3-seat allowance for Business.
pub fn license_claims(product_id: &str, major_version: u32, edition: Edition) -> Value {
    let mut v = json!({
        "schemaVersion": 1,
        "licenseId": format!("lic_test_{}", edition.id()),
        "customerId": "cus_test_001",
        "productId": product_id,
        "majorVersion": major_version,
        "edition": edition.id(),
        "issuedAt": "2026-01-01T00:00:00Z",
        "perpetual": true,
        "expiresAt": null,
    });
    if edition == Edition::Business {
        v["maxActivations"] = json!(3);
    }
    if edition != Edition::Community {
        v["organisation"] = json!("Example Organisation Pty Ltd");
    }
    v
}

/// Valid default activation-certificate claims with no expiry.
pub fn activation_claims(
    license_id: &str,
    product_id: &str,
    major_version: u32,
    device_id: &str,
) -> Value {
    json!({
        "schemaVersion": 1,
        "activationId": "act_test_001",
        "licenseId": license_id,
        "productId": product_id,
        "majorVersion": major_version,
        "deviceId": device_id,
        "issuedAt": "2026-01-02T00:00:00Z",
        "expiresAt": null,
    })
}
