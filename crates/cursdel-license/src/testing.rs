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

/// Deterministic, published test vectors for licensing-server
/// implementers (embedded in `docs/LICENSING-SERVER-BRIEF.md` and checked
/// against this crate by the `vectors` tests below). All three algorithms
/// sign deterministically here (Ed25519 per RFC 8032, ECDSA via RFC 6979,
/// ML-DSA-65 in its deterministic variant), so an implementation using
/// the same seeds and deterministic signing reproduces every byte;
/// randomised ECDSA/ML-DSA signers produce different, equally valid
/// signatures.
pub fn test_vectors() -> Value {
    let ed = TestSigner::new("vector-ed25519", SignatureAlgorithm::Ed25519, 1);
    let p256 = TestSigner::new("vector-p256", SignatureAlgorithm::EcdsaP256Sha256, 2);
    let pq = TestSigner::new("vector-mldsa65", SignatureAlgorithm::MlDsa65, 3);
    let key = |s: &TestSigner, seed: u8| {
        json!({
            "keyId": s.key_id,
            "algorithm": s.algorithm.id(),
            "testOnlyPrivateSeedHex": hex::encode([seed; 32]),
            "publicKey": URL_SAFE_NO_PAD.encode(s.public_key_bytes()),
        })
    };
    let envelope = |kind: EnvelopeKind, claims: &Value, signers: &[&TestSigner]| -> Value {
        serde_json::from_str(&seal(kind, claims, signers)).expect("sealed envelope is JSON")
    };
    let device_id = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";

    let enterprise = json!({
        "schemaVersion": 1,
        "licenseId": "lic_vector_enterprise",
        "customerId": "cus_vector_001",
        "productId": "cursedelete",
        "majorVersion": 2,
        "edition": "enterprise",
        "organisation": "Example Organisation Pty Ltd",
        "issuedAt": "2026-10-11T00:00:00Z",
        "perpetual": true,
        "expiresAt": null,
    });
    let business = json!({
        "schemaVersion": 1,
        "licenseId": "lic_vector_business",
        "customerId": "cus_vector_002",
        "productId": "cursedelete",
        "majorVersion": 2,
        "edition": "business",
        "organisation": "Example Business Pty Ltd",
        "issuedAt": "2026-10-11T00:00:00Z",
        "perpetual": false,
        "expiresAt": "2027-10-11T00:00:00Z",
        "maxActivations": 3,
    });
    let activation = json!({
        "schemaVersion": 1,
        "activationId": "act_vector_001",
        "licenseId": "lic_vector_business",
        "productId": "cursedelete",
        "majorVersion": 2,
        "deviceId": device_id,
        "issuedAt": "2026-10-11T00:05:00Z",
        "expiresAt": null,
    });
    let community = json!({
        "schemaVersion": 1,
        "licenseId": "lic_vector_community",
        "customerId": "cus_vector_003",
        "productId": "cursedelete",
        "majorVersion": 2,
        "edition": "community",
        "customerName": "Jane Citizen",
        "issuedAt": "2026-10-11T00:00:00Z",
        "perpetual": true,
        "expiresAt": null,
    });

    // Valid signature, but over different claims than the payload now holds.
    let mut tampered = envelope(EnvelopeKind::License, &enterprise, &[&ed]);
    tampered["payload"] = json!(URL_SAFE_NO_PAD.encode(
        enterprise
            .to_string()
            .replace("lic_vector_enterprise", "lic_vector_forged")
    ));

    json!({
        "validateAs": {
            "productId": "cursedelete",
            "majorVersion": 2,
            "now": "2026-10-12T00:00:00Z",
            "deviceId": device_id,
        },
        "keys": [key(&ed, 1), key(&p256, 2), key(&pq, 3)],
        "vectors": [
            {
                "name": "enterprise-perpetual-ed25519",
                "claims": enterprise,
                "envelope": envelope(EnvelopeKind::License, &enterprise, &[&ed]),
                "expect": "valid",
            },
            {
                "name": "business-subscription-ecdsa-p256",
                "claims": business,
                "envelope": envelope(EnvelopeKind::License, &business, &[&p256]),
                "expect": "valid",
            },
            {
                "name": "business-activation-certificate-ml-dsa-65",
                "activates": "business-subscription-ecdsa-p256",
                "claims": activation,
                "envelope": envelope(EnvelopeKind::Activation, &activation, &[&pq]),
                "expect": "valid",
            },
            {
                "name": "community-dual-signed-ed25519-ml-dsa-65",
                "claims": community,
                "envelope": envelope(EnvelopeKind::License, &community, &[&ed, &pq]),
                "expect": "valid",
            },
            {
                "name": "tampered-payload-must-be-rejected",
                "envelope": tampered,
                "expect": "invalidSignature",
            },
        ],
    })
}

#[cfg(test)]
mod vectors {
    use super::*;
    use crate::keys::TrustedKeySet;
    use crate::validate::{LicensePolicy, ProductIdentity, Rejection};

    const BRIEF: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/LICENSING-SERVER-BRIEF.md"
    );
    const BEGIN: &str = "<!-- BEGIN TEST VECTORS -->\n```json\n";
    const END: &str = "\n```\n<!-- END TEST VECTORS -->";

    fn embedded(doc: &str) -> (usize, usize) {
        let start = doc
            .find(BEGIN)
            .expect("brief has a BEGIN TEST VECTORS marker")
            + BEGIN.len();
        let end = start + doc[start..].find(END).expect("brief has an END marker");
        (start, end)
    }

    /// Run with `UPDATE_LICENSING_VECTORS=1` to rewrite the block in the brief.
    #[test]
    fn brief_contains_the_current_vectors() {
        let expected = serde_json::to_string_pretty(&test_vectors()).unwrap();
        let doc = std::fs::read_to_string(BRIEF).unwrap();
        let (start, end) = embedded(&doc);
        if std::env::var_os("UPDATE_LICENSING_VECTORS").is_some() {
            let updated = format!("{}{}{}", &doc[..start], expected, &doc[end..]);
            std::fs::write(BRIEF, updated).unwrap();
            return;
        }
        assert!(
            doc[start..end] == expected,
            "test vectors in docs/LICENSING-SERVER-BRIEF.md are stale; \
             rerun with UPDATE_LICENSING_VECTORS=1"
        );
    }

    #[test]
    fn every_vector_validates_as_documented() {
        let v = test_vectors();
        let keys = v["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| {
                TrustedKey::from_base64url(
                    k["keyId"].as_str().unwrap(),
                    SignatureAlgorithm::from_id(k["algorithm"].as_str().unwrap()).unwrap(),
                    k["publicKey"].as_str().unwrap(),
                )
                .unwrap()
            })
            .collect();
        let policy = LicensePolicy {
            product: ProductIdentity {
                product_id: "cursedelete".into(),
                major_version: 2,
            },
            trusted_keys: TrustedKeySet::new(keys).unwrap(),
            allowed_algorithms: SignatureAlgorithm::ALL.to_vec(),
            business_offline_limit: None,
        };
        let now = time::macros::datetime!(2026-10-12 0:00 UTC);
        let device = v["validateAs"]["deviceId"].as_str().unwrap();
        let vectors = v["vectors"].as_array().unwrap();
        let text = |name: &str| {
            vectors.iter().find(|x| x["name"] == name).unwrap()["envelope"].to_string()
        };

        for vector in vectors {
            let name = vector["name"].as_str().unwrap();
            let result = match vector["activates"].as_str() {
                Some(licence) => {
                    let licence = policy.verify_license(&text(licence), now).unwrap();
                    policy
                        .verify_activation(&text(name), &licence.claims, device, now)
                        .map(|_| ())
                }
                None => policy.verify_license(&text(name), now).map(|_| ()),
            };
            match vector["expect"].as_str().unwrap() {
                "valid" => assert!(result.is_ok(), "{name}: {result:?}"),
                "invalidSignature" => assert!(
                    matches!(result, Err(Rejection::InvalidSignature(_))),
                    "{name}: {result:?}"
                ),
                other => panic!("unknown expectation {other}"),
            }
        }
    }
}
