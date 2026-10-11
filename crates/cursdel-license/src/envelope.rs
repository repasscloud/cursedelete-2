//! The signed envelope that carries a licence or an activation
//! certificate.
//!
//! ```json
//! {
//!   "format": "repass-license-v1",
//!   "payload": "<base64url of the UTF-8 JSON claims>",
//!   "signatures": [
//!     { "signatureAlgorithm": "Ed25519", "keyId": "repass-signing-alpha", "signature": "<base64url>" }
//!   ]
//! }
//! ```
//!
//! **Signed bytes.** Each signature covers exactly
//! `UTF8(format) || 0x0A || payload_bytes`, where `payload_bytes` is the
//! base64url-decoded `payload`. Nothing is re-serialised before
//! verification, so JSON formatting and property order never matter, and
//! the `format` prefix stops a licence signature being replayed as an
//! activation certificate (or vice versa). All base64 is base64url
//! without padding.
//!
//! **Multiple signatures.** An envelope may carry several signatures
//! (e.g. Ed25519 + ML-DSA-65 during a migration). Verification succeeds
//! when at least one signature is from a trusted key using an allowed
//! algorithm and verifies. A signature from a trusted, allowed key that
//! does *not* verify fails the whole envelope -- that means tampering or
//! a signing bug, never something to paper over with another signature.
//! Signatures this build cannot use (unknown algorithm, disallowed
//! algorithm, untrusted key) are skipped, which is what lets older builds
//! accept a dual-signed licence.

use std::collections::HashSet;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::algorithm::SignatureAlgorithm;
use crate::keys::TrustedKeySet;

/// Envelopes are small; ML-DSA-65 signatures are ~4.4 KB in base64url.
/// Anything far larger is not a licence and is rejected before parsing.
const MAX_ENVELOPE_BYTES: usize = 64 * 1024;
const MAX_SIGNATURES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeKind {
    License,
    Activation,
}

impl EnvelopeKind {
    pub fn format(self) -> &'static str {
        match self {
            EnvelopeKind::License => "repass-license-v1",
            EnvelopeKind::Activation => "repass-activation-v1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("{0}")]
    Malformed(String),
    #[error("unsupported format '{0}'")]
    UnsupportedFormat(String),
    #[error("signed only with unsupported algorithm(s): {}", .0.join(", "))]
    UnsupportedAlgorithm(Vec<String>),
    #[error("signed only with algorithm(s) this application does not permit: {}", .0.join(", "))]
    DisallowedAlgorithm(Vec<String>),
    #[error("signed only by key(s) this build does not trust: {}", .0.join(", "))]
    UntrustedKey(Vec<String>),
    #[error("signature by key '{0}' is invalid")]
    InvalidSignature(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSignature {
    pub key_id: String,
    pub algorithm: SignatureAlgorithm,
}

#[derive(Debug, Clone)]
pub struct VerifiedPayload {
    pub payload: Vec<u8>,
    pub verified_by: Vec<VerifiedSignature>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct RawEnvelope {
    pub format: String,
    pub payload: String,
    pub signatures: Vec<RawSignature>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct RawSignature {
    pub signature_algorithm: String,
    pub key_id: String,
    pub signature: String,
}

pub(crate) fn signed_message(kind: EnvelopeKind, payload: &[u8]) -> Vec<u8> {
    let format = kind.format().as_bytes();
    let mut message = Vec::with_capacity(format.len() + 1 + payload.len());
    message.extend_from_slice(format);
    message.push(b'\n');
    message.extend_from_slice(payload);
    message
}

/// Parse and verify an envelope, returning its payload bytes only if a
/// trusted, allowed signature covers them.
pub fn open(
    text: &str,
    kind: EnvelopeKind,
    keys: &TrustedKeySet,
    allowed: &[SignatureAlgorithm],
) -> Result<VerifiedPayload, EnvelopeError> {
    if text.len() > MAX_ENVELOPE_BYTES {
        return Err(EnvelopeError::Malformed(format!(
            "file is larger than {MAX_ENVELOPE_BYTES} bytes"
        )));
    }
    let raw: RawEnvelope = serde_json::from_str(text)
        .map_err(|e| EnvelopeError::Malformed(format!("not a signed licence envelope: {e}")))?;
    if raw.format != kind.format() {
        return Err(EnvelopeError::UnsupportedFormat(raw.format));
    }
    let payload = URL_SAFE_NO_PAD
        .decode(raw.payload.as_bytes())
        .map_err(|_| EnvelopeError::Malformed("payload is not valid base64url".into()))?;
    if raw.signatures.is_empty() || raw.signatures.len() > MAX_SIGNATURES {
        return Err(EnvelopeError::Malformed(format!(
            "envelope must carry between 1 and {MAX_SIGNATURES} signatures"
        )));
    }
    let mut key_ids = HashSet::new();
    if let Some(dup) = raw
        .signatures
        .iter()
        .find(|s| !key_ids.insert(s.key_id.as_str()))
    {
        return Err(EnvelopeError::Malformed(format!(
            "key '{}' signs the envelope more than once",
            dup.key_id
        )));
    }

    let message = signed_message(kind, &payload);
    let mut verified_by = Vec::new();
    let mut unsupported = Vec::new();
    let mut disallowed = Vec::new();
    let mut untrusted = Vec::new();

    for sig in &raw.signatures {
        let Some(algorithm) = SignatureAlgorithm::from_id(&sig.signature_algorithm) else {
            unsupported.push(sig.signature_algorithm.clone());
            continue;
        };
        if !allowed.contains(&algorithm) {
            disallowed.push(algorithm.id().to_string());
            continue;
        }
        let Some(key) = keys.get(&sig.key_id) else {
            untrusted.push(sig.key_id.clone());
            continue;
        };
        if key.algorithm() != algorithm {
            return Err(EnvelopeError::Malformed(format!(
                "signature claims key '{}' is {algorithm}, but that key is {}",
                key.id(),
                key.algorithm()
            )));
        }
        let signature = URL_SAFE_NO_PAD
            .decode(sig.signature.as_bytes())
            .map_err(|_| EnvelopeError::Malformed("signature is not valid base64url".into()))?;
        if !key.verify(&message, &signature) {
            return Err(EnvelopeError::InvalidSignature(sig.key_id.clone()));
        }
        verified_by.push(VerifiedSignature {
            key_id: sig.key_id.clone(),
            algorithm,
        });
    }

    if !verified_by.is_empty() {
        Ok(VerifiedPayload {
            payload,
            verified_by,
        })
    } else if !untrusted.is_empty() {
        Err(EnvelopeError::UntrustedKey(untrusted))
    } else if !disallowed.is_empty() {
        Err(EnvelopeError::DisallowedAlgorithm(disallowed))
    } else {
        Err(EnvelopeError::UnsupportedAlgorithm(unsupported))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{seal_bytes, TestSigner};

    const ALL: &[SignatureAlgorithm] = &SignatureAlgorithm::ALL;

    fn keys(signers: &[&TestSigner]) -> TrustedKeySet {
        TrustedKeySet::new(signers.iter().map(|s| s.trusted_key()).collect()).unwrap()
    }

    #[test]
    fn each_algorithm_verifies_on_its_own() {
        for alg in SignatureAlgorithm::ALL {
            let signer = TestSigner::new("k1", alg, 3);
            let text = seal_bytes(EnvelopeKind::License, b"{\"a\":1}", &[&signer]);
            let opened = open(&text, EnvelopeKind::License, &keys(&[&signer]), ALL).unwrap();
            assert_eq!(opened.payload, b"{\"a\":1}");
            assert_eq!(opened.verified_by[0].algorithm, alg);
        }
    }

    #[test]
    fn tampered_payload_fails_for_every_algorithm() {
        for alg in SignatureAlgorithm::ALL {
            let signer = TestSigner::new("k1", alg, 3);
            let text = seal_bytes(
                EnvelopeKind::License,
                b"{\"edition\":\"community\"}",
                &[&signer],
            );
            let mut raw: RawEnvelope = serde_json::from_str(&text).unwrap();
            raw.payload = URL_SAFE_NO_PAD.encode(b"{\"edition\":\"enterprise\"}");
            let tampered = serde_json::to_string(&raw).unwrap();
            assert_eq!(
                open(&tampered, EnvelopeKind::License, &keys(&[&signer]), ALL).unwrap_err(),
                EnvelopeError::InvalidSignature("k1".into())
            );
        }
    }

    #[test]
    fn licence_signature_cannot_be_replayed_as_activation() {
        let signer = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&signer]);
        let mut raw: RawEnvelope = serde_json::from_str(&text).unwrap();
        raw.format = EnvelopeKind::Activation.format().into();
        let relabelled = serde_json::to_string(&raw).unwrap();
        assert_eq!(
            open(
                &relabelled,
                EnvelopeKind::Activation,
                &keys(&[&signer]),
                ALL
            )
            .unwrap_err(),
            EnvelopeError::InvalidSignature("k1".into())
        );
    }

    #[test]
    fn wrong_format_is_reported() {
        let signer = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let text = seal_bytes(EnvelopeKind::Activation, b"{}", &[&signer]);
        assert_eq!(
            open(&text, EnvelopeKind::License, &keys(&[&signer]), ALL).unwrap_err(),
            EnvelopeError::UnsupportedFormat("repass-activation-v1".into())
        );
    }

    #[test]
    fn untrusted_key_is_reported() {
        let signer = TestSigner::new("stranger", SignatureAlgorithm::Ed25519, 3);
        let trusted = TestSigner::new("ours", SignatureAlgorithm::Ed25519, 4);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&signer]);
        assert_eq!(
            open(&text, EnvelopeKind::License, &keys(&[&trusted]), ALL).unwrap_err(),
            EnvelopeError::UntrustedKey(vec!["stranger".into()])
        );
    }

    #[test]
    fn disallowed_algorithm_is_not_used_even_with_a_trusted_key() {
        let signer = TestSigner::new("k1", SignatureAlgorithm::EcdsaP256Sha256, 3);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&signer]);
        assert_eq!(
            open(
                &text,
                EnvelopeKind::License,
                &keys(&[&signer]),
                &[SignatureAlgorithm::Ed25519]
            )
            .unwrap_err(),
            EnvelopeError::DisallowedAlgorithm(vec!["ECDSA-P256-SHA256".into()])
        );
    }

    #[test]
    fn dual_signed_envelope_verifies_with_either_key_alone() {
        let ed = TestSigner::new("alpha-ed25519", SignatureAlgorithm::Ed25519, 3);
        let pq = TestSigner::new("alpha-mldsa65", SignatureAlgorithm::MlDsa65, 4);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&ed, &pq]);

        // An older build that only knows the Ed25519 key.
        let old = open(&text, EnvelopeKind::License, &keys(&[&ed]), ALL).unwrap();
        assert_eq!(old.verified_by.len(), 1);
        // A newer build that only trusts the post-quantum key.
        let new = open(&text, EnvelopeKind::License, &keys(&[&pq]), ALL).unwrap();
        assert_eq!(new.verified_by[0].algorithm, SignatureAlgorithm::MlDsa65);
        // A build trusting both verifies both.
        let both = open(&text, EnvelopeKind::License, &keys(&[&ed, &pq]), ALL).unwrap();
        assert_eq!(both.verified_by.len(), 2);
    }

    #[test]
    fn unknown_algorithm_alongside_a_good_signature_is_ignored() {
        let ed = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&ed]);
        let mut raw: RawEnvelope = serde_json::from_str(&text).unwrap();
        raw.signatures.push(RawSignature {
            signature_algorithm: "SLH-DSA-SHA2-128s".into(),
            key_id: "future".into(),
            signature: "AAAA".into(),
        });
        let text = serde_json::to_string(&raw).unwrap();
        assert!(open(&text, EnvelopeKind::License, &keys(&[&ed]), ALL).is_ok());
    }

    #[test]
    fn only_unknown_algorithms_is_reported() {
        let ed = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&ed]);
        let mut raw: RawEnvelope = serde_json::from_str(&text).unwrap();
        raw.signatures[0].signature_algorithm = "RSA-PSS".into();
        let text = serde_json::to_string(&raw).unwrap();
        assert_eq!(
            open(&text, EnvelopeKind::License, &keys(&[&ed]), ALL).unwrap_err(),
            EnvelopeError::UnsupportedAlgorithm(vec!["RSA-PSS".into()])
        );
    }

    #[test]
    fn a_bad_signature_from_a_trusted_key_fails_even_if_another_verifies() {
        let ed = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let p = TestSigner::new("k2", SignatureAlgorithm::EcdsaP256Sha256, 4);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&ed, &p]);
        let mut raw: RawEnvelope = serde_json::from_str(&text).unwrap();
        let other = seal_bytes(EnvelopeKind::License, b"{\"x\":1}", &[&p]);
        let other: RawEnvelope = serde_json::from_str(&other).unwrap();
        raw.signatures[1].signature = other.signatures[0].signature.clone();
        let text = serde_json::to_string(&raw).unwrap();
        assert_eq!(
            open(&text, EnvelopeKind::License, &keys(&[&ed, &p]), ALL).unwrap_err(),
            EnvelopeError::InvalidSignature("k2".into())
        );
    }

    #[test]
    fn key_algorithm_mismatch_is_rejected() {
        let ed = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&ed]);
        let mut raw: RawEnvelope = serde_json::from_str(&text).unwrap();
        raw.signatures[0].signature_algorithm = "ML-DSA-65".into();
        let text = serde_json::to_string(&raw).unwrap();
        assert!(matches!(
            open(&text, EnvelopeKind::License, &keys(&[&ed]), ALL),
            Err(EnvelopeError::Malformed(_))
        ));
    }

    #[test]
    fn corrupted_and_oversized_files_are_malformed() {
        let set = TrustedKeySet::default();
        for text in [
            "",
            "not json",
            "{\"format\":\"repass-license-v1\"}",
            "{\"format\":\"repass-license-v1\",\"payload\":\"e30\",\"signatures\":[]}",
        ] {
            assert!(matches!(
                open(text, EnvelopeKind::License, &set, ALL),
                Err(EnvelopeError::Malformed(_))
            ));
        }
        let huge = " ".repeat(MAX_ENVELOPE_BYTES + 1);
        assert!(matches!(
            open(&huge, EnvelopeKind::License, &set, ALL),
            Err(EnvelopeError::Malformed(_))
        ));
    }

    #[test]
    fn unknown_envelope_fields_are_rejected() {
        let ed = TestSigner::new("k1", SignatureAlgorithm::Ed25519, 3);
        let text = seal_bytes(EnvelopeKind::License, b"{}", &[&ed]);
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["edition"] = "enterprise".into();
        assert!(matches!(
            open(
                &value.to_string(),
                EnvelopeKind::License,
                &keys(&[&ed]),
                ALL
            ),
            Err(EnvelopeError::Malformed(_))
        ));
    }
}
