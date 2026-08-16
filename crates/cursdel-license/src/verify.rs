//! Offline, no-network licence verification -- mirrors
//! `src/Licensing.Core/LicenseVerifier.cs`. No public-key handling, no
//! network call, and no server dependency in this path; it is exactly the
//! self-contained verifier the `software-license-v1` envelope exists to
//! support.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde_json::{Map, Value};
use signature::Verifier;
use time::{Date, OffsetDateTime};

use crate::canonical_json;
use crate::device::{self, LocalDeviceIdentity};
use crate::error::ValidationError;
use crate::schema::{self, LicenseData, ProductEntitlement};
use crate::trusted_keys::trusted_public_keys;

pub const FORMAT: &str = "software-license-v1";
pub const ALGORITHM: &str = "ECDSA-P256-SHA256";

const ENVELOPE_FIELDS: &[&str] = &["format", "algorithm", "keyId", "license", "signature"];

#[derive(Debug, Clone)]
pub struct VerifiedLicense {
    pub key_id: String,
    pub data: LicenseData,
}

pub fn verify_file(path: &Path) -> Result<VerifiedLicense, ValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ValidationError::new(format!("could not read licence file: {e}")))?;
    verify(&text)
}

pub fn verify(signed_license_json: &str) -> Result<VerifiedLicense, ValidationError> {
    let trusted = trusted_public_keys();
    verify_with_keys(signed_license_json, &trusted)
}

pub fn verify_with_keys(
    signed_license_json: &str,
    trusted_public_keys_by_key_id: &HashMap<&str, &str>,
) -> Result<VerifiedLicense, ValidationError> {
    let root: Value = serde_json::from_str(signed_license_json)
        .map_err(|e| ValidationError::new(format!("Licence JSON is invalid: {e}")))?;
    let obj = root
        .as_object()
        .ok_or_else(|| ValidationError::new("Licence file must contain a JSON object."))?;

    ensure_envelope_fields(obj)?;

    let format = required_string(obj, "format")?;
    let algorithm = required_string(obj, "algorithm")?;
    let key_id = required_string(obj, "keyId")?;
    let signature_text = required_string(obj, "signature")?;

    if format != FORMAT {
        return Err(ValidationError::new(format!(
            "Unsupported licence format: {format}"
        )));
    }
    if algorithm != ALGORITHM {
        return Err(ValidationError::new(format!(
            "Unsupported algorithm: {algorithm}"
        )));
    }

    let public_key_pem = *trusted_public_keys_by_key_id
        .get(key_id.as_str())
        .ok_or_else(|| ValidationError::new(format!("Unknown signing key: {key_id}")))?;

    let license_value = obj
        .get("license")
        .and_then(Value::as_object)
        .ok_or_else(|| ValidationError::new("Licence payload is missing."))?;

    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_text.as_bytes())
        .map_err(|_| ValidationError::new("Signature is not valid Base64."))?;

    let mut unsigned = obj.clone();
    unsigned.remove("signature");
    let canonical_bytes = canonical_json::canonicalize(&Value::Object(unsigned));

    verify_signature(public_key_pem, &canonical_bytes, &signature_bytes)
        .map_err(|_| ValidationError::new("Cryptographic signature is invalid."))?;

    let data = schema::parse(&Value::Object(license_value.clone()))?;

    Ok(VerifiedLicense { key_id, data })
}

fn verify_signature(pem: &str, message: &[u8], signature_bytes: &[u8]) -> Result<(), ()> {
    let public_key = p256::PublicKey::from_public_key_pem(pem).map_err(|_| ())?;
    let verifying_key = VerifyingKey::from(&public_key);
    // IEEE P1363 fixed-field concatenation: raw r||s, 32 bytes each for
    // P-256 -- not DER. This matches the server's
    // DSASignatureFormat.IeeeP1363FixedFieldConcatenation exactly.
    let sig = Signature::from_slice(signature_bytes).map_err(|_| ())?;
    verifying_key.verify(message, &sig).map_err(|_| ())
}

fn ensure_envelope_fields(obj: &Map<String, Value>) -> Result<(), ValidationError> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for key in obj.keys() {
        let lower = key.to_ascii_lowercase();
        if !seen.insert(lower) {
            return Err(ValidationError::new(format!(
                "Licence envelope contains an ambiguous field: '{key}'."
            )));
        }
        if !ENVELOPE_FIELDS.contains(&key.as_str()) {
            return Err(ValidationError::new(format!(
                "Licence envelope contains an unsupported field: '{key}'."
            )));
        }
    }
    Ok(())
}

fn required_string(obj: &Map<String, Value>, name: &str) -> Result<String, ValidationError> {
    match obj.get(name).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
        _ => Err(ValidationError::new(format!(
            "Licence does not contain a valid {name}."
        ))),
    }
}

pub fn validate_product(
    verified: &VerifiedLicense,
    product: &str,
    release_date: Option<Date>,
    current_time_utc: Option<OffsetDateTime>,
    current_device: Option<&LocalDeviceIdentity>,
) -> Result<ProductEntitlement, ValidationError> {
    validate_activation(verified, current_device, current_time_utc)?;

    let entitlement = verified
        .data
        .entitlements
        .iter()
        .find(|e| e.product.eq_ignore_ascii_case(product))
        .cloned()
        .ok_or_else(|| {
            ValidationError::new(format!(
                "Licence does not contain an entitlement for '{product}'."
            ))
        })?;

    let now = current_time_utc.unwrap_or_else(OffsetDateTime::now_utc);

    if let Some(expires_at) = entitlement.expires_at {
        if now >= expires_at {
            return Err(ValidationError::new(format!(
                "Entitlement for '{}' expired at {}.",
                entitlement.product, expires_at
            )));
        }
    }

    if let (Some(release_date), Some(updates_until)) = (release_date, entitlement.updates_until) {
        if release_date > updates_until {
            return Err(ValidationError::new(format!(
                "Entitlement for '{}' includes releases through {}; release {} is not covered.",
                entitlement.product, updates_until, release_date
            )));
        }
    }

    Ok(entitlement)
}

pub fn validate_activation(
    verified: &VerifiedLicense,
    current_device: Option<&LocalDeviceIdentity>,
    current_time_utc: Option<OffsetDateTime>,
) -> Result<(), ValidationError> {
    let binding = &verified.data.device_binding;
    let activation = &verified.data.activation;

    match (binding, activation) {
        (None, None) => return Ok(()),
        (Some(_), None) | (None, Some(_)) => {
            return Err(ValidationError::new(
                "Licence activation data is incomplete.",
            ))
        }
        _ => {}
    }

    let binding = binding.as_ref().unwrap();
    let activation = activation.as_ref().unwrap();

    let owned_device;
    let device = match current_device {
        Some(d) => d,
        None => {
            owned_device = device::current();
            &owned_device
        }
    };

    let device_matches = binding.scheme == device.scheme
        && constant_time_eq(binding.device_id.as_bytes(), device.device_id.as_bytes());

    if !device_matches {
        let suffix_a = &binding.device_id[binding.device_id.len().saturating_sub(8)..];
        let suffix_b = &device.device_id[device.device_id.len().saturating_sub(8)..];
        return Err(ValidationError::new(format!(
            "Licence is activated for a different device (expected ...{suffix_a}, this device is ...{suffix_b}). Deactivate it before transferring."
        )));
    }

    let now = current_time_utc.unwrap_or_else(OffsetDateTime::now_utc);
    if let Some(lease) = activation.lease_expires_at {
        if now >= lease {
            return Err(ValidationError::new(format!(
                "Activation lease expired at {lease}; connect to refresh it."
            )));
        }
    }

    Ok(())
}

/// Fixed-time byte comparison, matching the server's use of
/// `CryptographicOperations.FixedTimeEquals` for the device-id compare.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn test_keys() -> HashMap<&'static str, &'static str> {
        let mut m = HashMap::new();
        m.insert(
            "test2026",
            Box::leak(fixture("test2026.public.pem").into_boxed_str()) as &'static str,
        );
        m
    }

    #[test]
    fn verifies_real_dotnet_signed_license() {
        let license_json = fixture("test.license");
        let result = verify_with_keys(&license_json, &test_keys());
        let verified = result.expect("real C#-signed license must verify");
        assert_eq!(verified.key_id, "test2026");
        assert_eq!(verified.data.license_id, "LIC-TEST-0001");
        assert_eq!(verified.data.entitlements.len(), 1);
        assert_eq!(verified.data.entitlements[0].product, "cursedelete");
    }

    #[test]
    fn verifies_exhaustive_character_coverage_license() {
        let license_json = fixture("exhaustive.license");
        let result = verify_with_keys(&license_json, &test_keys());
        assert!(
            result.is_ok(),
            "exhaustive character-coverage license must verify: {result:?}"
        );
    }

    #[test]
    fn rejects_tampered_payload() {
        let license_json = fixture("test.license");
        let mut value: Value = serde_json::from_str(&license_json).unwrap();
        value["license"]["customer"] = Value::String("Attacker Pty Ltd".to_string());
        let tampered = serde_json::to_string(&value).unwrap();
        let result = verify_with_keys(&tampered, &test_keys());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_signing_key() {
        let license_json = fixture("test.license");
        let empty_keys: HashMap<&str, &str> = HashMap::new();
        let result = verify_with_keys(&license_json, &empty_keys);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_wrong_format() {
        let license_json = fixture("test.license");
        let mut value: Value = serde_json::from_str(&license_json).unwrap();
        value["format"] = Value::String("something-else".to_string());
        let result = verify_with_keys(&serde_json::to_string(&value).unwrap(), &test_keys());
        assert!(result.is_err());
    }

    #[test]
    fn validate_product_finds_entitlement_case_insensitively() {
        let license_json = fixture("test.license");
        let verified = verify_with_keys(&license_json, &test_keys()).unwrap();
        let entitlement = validate_product(&verified, "CurseDelete", None, None, None);
        assert!(entitlement.is_ok());
    }

    #[test]
    fn validate_product_rejects_missing_entitlement() {
        let license_json = fixture("test.license");
        let verified = verify_with_keys(&license_json, &test_keys()).unwrap();
        let entitlement = validate_product(&verified, "some-other-product", None, None, None);
        assert!(entitlement.is_err());
    }

    #[test]
    fn validate_product_rejects_expired_entitlement() {
        // A perpetual license with no expiresAt should never expire; build
        // one with an explicit past expiry to exercise the check.
        let license_json = fixture("test.license");
        let mut value: Value = serde_json::from_str(&license_json).unwrap();
        value["license"]["entitlements"][0]["expiresAt"] =
            Value::String("2020-01-01T00:00:00Z".to_string());
        // Re-signing isn't possible without the private key, so instead
        // directly exercise validate_product against schema-parsed data
        // built in-process (bypassing signature verification) to isolate
        // the expiry check itself.
        let license_obj = value["license"].as_object().unwrap().clone();
        let data = schema::parse(&Value::Object(license_obj)).unwrap();
        let verified = VerifiedLicense {
            key_id: "test2026".to_string(),
            data,
        };
        let result = validate_product(&verified, "cursedelete", None, None, None);
        assert!(result.is_err());
    }

    fn license_with_activation(mode: &str, lease_expires_at: Option<&str>) -> Value {
        let license_json = fixture("test.license");
        let mut value: Value = serde_json::from_str(&license_json).unwrap();
        value["license"]["deviceBinding"] = json!({
            "scheme": "os-machine-id-sha256-v1",
            "deviceId": "A".repeat(64),
            "deviceName": "original-device",
        });
        let mut activation = serde_json::Map::new();
        activation.insert("activationId".to_string(), json!("act-1"));
        activation.insert("mode".to_string(), json!(mode));
        activation.insert("activatedAt".to_string(), json!("2026-01-01T00:00:00Z"));
        if mode == "online" {
            activation.insert("refreshAfter".to_string(), json!("2026-01-02T00:00:00Z"));
            activation.insert(
                "leaseExpiresAt".to_string(),
                json!(lease_expires_at.unwrap_or("2026-01-08T00:00:00Z")),
            );
        }
        value["license"]["activation"] = Value::Object(activation);
        value
    }

    fn verified_from(value: &Value) -> VerifiedLicense {
        let license_obj = value["license"].as_object().unwrap().clone();
        let data = schema::parse(&Value::Object(license_obj)).unwrap();
        VerifiedLicense {
            key_id: "test2026".to_string(),
            data,
        }
    }

    #[test]
    fn validate_activation_rejects_a_different_device() {
        let value = license_with_activation("offline", None);
        let verified = verified_from(&value);

        let other_device = LocalDeviceIdentity {
            scheme: "os-machine-id-sha256-v1",
            device_id: "B".repeat(64),
            device_name: "someone-elses-laptop".to_string(),
            source: "test",
        };

        let result = validate_activation(&verified, Some(&other_device), None);
        let err = result.expect_err("a license bound to a different device must be rejected");
        assert!(
            err.to_string().contains("different device"),
            "error should explain the device mismatch, got: {err}"
        );
    }

    #[test]
    fn validate_activation_accepts_the_matching_device() {
        let value = license_with_activation("offline", None);
        let verified = verified_from(&value);

        let matching_device = LocalDeviceIdentity {
            scheme: "os-machine-id-sha256-v1",
            device_id: "A".repeat(64),
            device_name: "original-device".to_string(),
            source: "test",
        };

        assert!(validate_activation(&verified, Some(&matching_device), None).is_ok());
    }

    #[test]
    fn validate_activation_rejects_an_expired_online_lease() {
        let value = license_with_activation("online", Some("2026-01-08T00:00:00Z"));
        let verified = verified_from(&value);

        let matching_device = LocalDeviceIdentity {
            scheme: "os-machine-id-sha256-v1",
            device_id: "A".repeat(64),
            device_name: "original-device".to_string(),
            source: "test",
        };

        // "Now" is after the lease expiry.
        let now = time::macros::datetime!(2026-02-01 0:00 UTC);
        let result = validate_activation(&verified, Some(&matching_device), Some(now));
        let err = result.expect_err("an expired activation lease must be rejected");
        assert!(
            err.to_string().contains("lease expired"),
            "error should explain the lease expired, got: {err}"
        );
    }

    #[test]
    fn validate_activation_accepts_offline_mode_with_no_lease_to_expire() {
        // Offline activations have no refreshAfter/leaseExpiresAt at all
        // (LICENSING-INTEGRATION.md §2.3: "this produces an activation
        // with no lease ... so it never needs to phone home again"), so
        // it must remain valid indefinitely regardless of how far in the
        // future "now" is.
        let value = license_with_activation("offline", None);
        let verified = verified_from(&value);
        let matching_device = LocalDeviceIdentity {
            scheme: "os-machine-id-sha256-v1",
            device_id: "A".repeat(64),
            device_name: "original-device".to_string(),
            source: "test",
        };
        let far_future = time::macros::datetime!(2099-01-01 0:00 UTC);
        assert!(validate_activation(&verified, Some(&matching_device), Some(far_future)).is_ok());
    }

    #[test]
    fn constant_time_eq_matches_and_mismatches() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
