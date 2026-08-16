//! Strict licence payload schema, mirroring
//! `src/Licensing.Core/LicenseSchema.cs` field-for-field and rule-for-rule:
//! unknown top-level fields rejected, case-insensitive duplicate field
//! names rejected, `deviceBinding`/`activation` both-or-neither,
//! non-empty `entitlements` with unique `product` values, ISO-8601
//! timestamps with a mandatory explicit offset, and `metadata` limited to
//! lowerCamelCase string/number/boolean scalars.

use std::collections::HashSet;

use serde_json::{Map, Value};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::device::DEVICE_SCHEME;
use crate::error::SchemaError;

#[derive(Debug, Clone)]
pub struct ProductEntitlement {
    pub product: String,
    pub edition: String,
    pub license_type: String,
    pub seats: i64,
    pub expires_at: Option<OffsetDateTime>,
    pub updates_until: Option<Date>,
}

#[derive(Debug, Clone)]
pub struct DeviceBinding {
    pub scheme: String,
    pub device_id: String,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActivationData {
    pub activation_id: String,
    pub mode: String,
    pub activated_at: OffsetDateTime,
    pub refresh_after: Option<OffsetDateTime>,
    pub lease_expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct LicenseData {
    pub license_id: String,
    pub customer: String,
    pub issued_at: OffsetDateTime,
    pub metadata: Option<Map<String, Value>>,
    pub device_binding: Option<DeviceBinding>,
    pub activation: Option<ActivationData>,
    pub entitlements: Vec<ProductEntitlement>,
}

const LICENSE_FIELDS: &[&str] = &[
    "licenseId",
    "customer",
    "issuedAt",
    "metadata",
    "deviceBinding",
    "activation",
    "entitlements",
];
const ENTITLEMENT_FIELDS: &[&str] = &[
    "product",
    "edition",
    "licenseType",
    "seats",
    "expiresAt",
    "updatesUntil",
];
const DEVICE_BINDING_FIELDS: &[&str] = &["scheme", "deviceId", "deviceName"];
const ACTIVATION_FIELDS: &[&str] = &[
    "activationId",
    "mode",
    "activatedAt",
    "refreshAfter",
    "leaseExpiresAt",
];

pub fn parse(license: &Value) -> Result<LicenseData, SchemaError> {
    let obj = license
        .as_object()
        .ok_or_else(|| SchemaError::new("Licence payload must be a JSON object."))?;

    ensure_unambiguous_names(obj, "licence")?;
    ensure_only_allowed_fields(obj, LICENSE_FIELDS, "licence")?;

    let license_id = required_string(obj, "licenseId", "licence")?;
    let customer = required_string(obj, "customer", "licence")?;
    let issued_at = required_instant(obj, "issuedAt", "licence")?;
    let metadata = optional_metadata(obj)?;
    let device_binding = optional_device_binding(obj)?;
    let activation = optional_activation(obj)?;

    if device_binding.is_some() != activation.is_some() {
        return Err(SchemaError::new(
            "Licence fields 'deviceBinding' and 'activation' must either both be present or both be absent.",
        ));
    }

    let entitlements = match obj.get("entitlements").and_then(Value::as_array) {
        Some(arr) if !arr.is_empty() => arr,
        _ => {
            return Err(SchemaError::new(
                "Licence field 'entitlements' must be a non-empty array.",
            ))
        }
    };

    let mut parsed_entitlements = Vec::with_capacity(entitlements.len());
    let mut products: HashSet<String> = HashSet::new();

    for (index, entry) in entitlements.iter().enumerate() {
        let context = format!("entitlement at index {index}");
        let entry_obj = entry.as_object().ok_or_else(|| {
            SchemaError::new(format!(
                "Entitlement at index {index} must be a JSON object."
            ))
        })?;

        ensure_unambiguous_names(entry_obj, &context)?;
        ensure_only_allowed_fields(entry_obj, ENTITLEMENT_FIELDS, &context)?;

        let product = required_string(entry_obj, "product", &context)?;
        if !products.insert(product.to_ascii_lowercase()) {
            return Err(SchemaError::new(format!(
                "Licence contains more than one entitlement for product '{product}'."
            )));
        }

        parsed_entitlements.push(ProductEntitlement {
            product,
            edition: required_string(entry_obj, "edition", &context)?,
            license_type: required_string(entry_obj, "licenseType", &context)?,
            seats: required_positive_integer(entry_obj, "seats", &context)?,
            expires_at: optional_instant(entry_obj, "expiresAt", &context)?,
            updates_until: optional_date(entry_obj, "updatesUntil", &context)?,
        });
    }

    Ok(LicenseData {
        license_id,
        customer,
        issued_at,
        metadata,
        device_binding,
        activation,
        entitlements: parsed_entitlements,
    })
}

fn optional_metadata(
    license: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>, SchemaError> {
    let Some(value) = license.get("metadata") else {
        return Ok(None);
    };
    let map = value.as_object().ok_or_else(|| {
        SchemaError::new("Optional licence field 'metadata' must be a JSON object.")
    })?;

    ensure_unambiguous_names(map, "licence metadata")?;

    for (key, val) in map {
        if !is_lower_camel_case(key) {
            return Err(SchemaError::new(format!(
                "Metadata field '{key}' must use lowerCamelCase and contain only ASCII letters and digits."
            )));
        }
        if !matches!(val, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
            return Err(SchemaError::new(format!(
                "Metadata field '{key}' must be a string, number, or boolean. Nested objects, arrays, and null are not supported."
            )));
        }
    }

    Ok(Some(map.clone()))
}

fn optional_device_binding(
    license: &Map<String, Value>,
) -> Result<Option<DeviceBinding>, SchemaError> {
    let Some(value) = license.get("deviceBinding") else {
        return Ok(None);
    };
    let binding = value.as_object().ok_or_else(|| {
        SchemaError::new("Optional licence field 'deviceBinding' must be a JSON object.")
    })?;

    ensure_unambiguous_names(binding, "device binding")?;
    ensure_only_allowed_fields(binding, DEVICE_BINDING_FIELDS, "device binding")?;

    let scheme = required_string(binding, "scheme", "device binding")?;
    if scheme != DEVICE_SCHEME {
        return Err(SchemaError::new(format!(
            "Unsupported device-binding scheme: '{scheme}'."
        )));
    }

    let device_id = required_string(binding, "deviceId", "device binding")?;
    if !is_valid_device_id(&device_id) {
        return Err(SchemaError::new(
            "Device binding field 'deviceId' must be a 64-character SHA-256 hexadecimal value.",
        ));
    }

    Ok(Some(DeviceBinding {
        scheme,
        device_id: device_id.to_ascii_uppercase(),
        device_name: optional_string(binding, "deviceName", "device binding")?,
    }))
}

fn optional_activation(
    license: &Map<String, Value>,
) -> Result<Option<ActivationData>, SchemaError> {
    let Some(value) = license.get("activation") else {
        return Ok(None);
    };
    let activation = value.as_object().ok_or_else(|| {
        SchemaError::new("Optional licence field 'activation' must be a JSON object.")
    })?;

    ensure_unambiguous_names(activation, "activation")?;
    ensure_only_allowed_fields(activation, ACTIVATION_FIELDS, "activation")?;

    let mode = required_string(activation, "mode", "activation")?;
    if mode != "online" && mode != "offline" {
        return Err(SchemaError::new(
            "Activation field 'mode' must be either 'online' or 'offline'.",
        ));
    }

    let activated_at = required_instant(activation, "activatedAt", "activation")?;
    let refresh_after = optional_instant(activation, "refreshAfter", "activation")?;
    let lease_expires_at = optional_instant(activation, "leaseExpiresAt", "activation")?;

    if mode == "online" && (refresh_after.is_none() || lease_expires_at.is_none()) {
        return Err(SchemaError::new(
            "Online activations require both 'refreshAfter' and 'leaseExpiresAt'.",
        ));
    }

    if let Some(ra) = refresh_after {
        if ra <= activated_at {
            return Err(SchemaError::new(
                "Activation 'refreshAfter' must be after 'activatedAt'.",
            ));
        }
    }

    if let Some(le) = lease_expires_at {
        if le <= activated_at || refresh_after.is_some_and(|ra| le <= ra) {
            return Err(SchemaError::new(
                "Activation 'leaseExpiresAt' must be after 'activatedAt' and 'refreshAfter'.",
            ));
        }
    }

    Ok(Some(ActivationData {
        activation_id: required_string(activation, "activationId", "activation")?,
        mode,
        activated_at,
        refresh_after,
        lease_expires_at,
    }))
}

fn required_string(
    obj: &Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<String, SchemaError> {
    match obj.get(name).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
        _ => Err(SchemaError::new(format!(
            "Required field '{name}' in {context} must be a non-empty string."
        ))),
    }
}

fn optional_string(
    obj: &Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<Option<String>, SchemaError> {
    match obj.get(name) {
        None => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(_) => Err(SchemaError::new(format!(
            "Optional field '{name}' in {context} must be a non-empty string when present."
        ))),
    }
}

fn required_positive_integer(
    obj: &Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<i64, SchemaError> {
    match obj.get(name).and_then(Value::as_i64) {
        Some(n) if n > 0 => Ok(n),
        _ => Err(SchemaError::new(format!(
            "Required field '{name}' in {context} must be a positive integer."
        ))),
    }
}

fn required_instant(
    obj: &Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<OffsetDateTime, SchemaError> {
    match obj.get(name) {
        Some(v) => parse_instant(v, name, context),
        None => Err(SchemaError::new(format!(
            "Required field '{name}' is missing from {context}."
        ))),
    }
}

fn optional_instant(
    obj: &Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<Option<OffsetDateTime>, SchemaError> {
    match obj.get(name) {
        Some(v) => Ok(Some(parse_instant(v, name, context)?)),
        None => Ok(None),
    }
}

fn parse_instant(value: &Value, name: &str, context: &str) -> Result<OffsetDateTime, SchemaError> {
    let text = value.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        SchemaError::new(format!(
            "Field '{name}' in {context} must be an ISO-8601 timestamp with a timezone, such as 2030-12-31T23:59:59Z."
        ))
    })?;

    if !has_explicit_offset(text) {
        return Err(SchemaError::new(format!(
            "Field '{name}' in {context} must be an ISO-8601 timestamp with a timezone, such as 2030-12-31T23:59:59Z."
        )));
    }

    OffsetDateTime::parse(text, &Rfc3339)
        .map(|dt| dt.to_offset(time::UtcOffset::UTC))
        .map_err(|_| {
            SchemaError::new(format!(
                "Field '{name}' in {context} must be an ISO-8601 timestamp with a timezone, such as 2030-12-31T23:59:59Z."
            ))
        })
}

fn optional_date(
    obj: &Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<Option<Date>, SchemaError> {
    let Some(value) = obj.get(name) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            SchemaError::new(format!(
                "Field '{name}' in {context} must use the ISO date format yyyy-MM-dd."
            ))
        })?;
    parse_date(text, &format!("Field '{name}' in {context}")).map(Some)
}

pub fn parse_date(value: &str, field_name: &str) -> Result<Date, SchemaError> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(value, format).map_err(|_| {
        SchemaError::new(format!(
            "{field_name} must use the ISO date format yyyy-MM-dd."
        ))
    })
}

fn has_explicit_offset(value: &str) -> bool {
    if value.ends_with('Z') || value.ends_with('z') {
        return true;
    }
    if value.len() < 6 {
        return false;
    }
    let bytes = value.as_bytes();
    let offset_start = value.len() - 6;
    let sign = bytes[offset_start];
    (sign == b'+' || sign == b'-')
        && bytes[offset_start + 1].is_ascii_digit()
        && bytes[offset_start + 2].is_ascii_digit()
        && bytes[offset_start + 3] == b':'
        && bytes[offset_start + 4].is_ascii_digit()
        && bytes[offset_start + 5].is_ascii_digit()
}

fn ensure_unambiguous_names(obj: &Map<String, Value>, context: &str) -> Result<(), SchemaError> {
    let mut seen: HashSet<String> = HashSet::new();
    for key in obj.keys() {
        let lower = key.to_ascii_lowercase();
        if !seen.insert(lower) {
            return Err(SchemaError::new(format!(
                "{context} contains ambiguous field names that differ only by case: '{key}'."
            )));
        }
    }
    Ok(())
}

fn ensure_only_allowed_fields(
    obj: &Map<String, Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), SchemaError> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(SchemaError::new(format!(
                "Unsupported field '{key}' in {context}. Custom fields are allowed only inside licence metadata."
            )));
        }
    }
    Ok(())
}

fn is_lower_camel_case(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.chars().all(|c| c.is_ascii_alphanumeric())
}

pub fn is_valid_device_id(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_license() -> Value {
        json!({
            "licenseId": "LIC-1",
            "customer": "Acme",
            "issuedAt": "2026-01-01T00:00:00Z",
            "entitlements": [
                {"product": "cursedelete", "edition": "business", "licenseType": "perpetual", "seats": 5}
            ]
        })
    }

    #[test]
    fn parses_minimal_valid_license() {
        let data = parse(&valid_license()).unwrap();
        assert_eq!(data.license_id, "LIC-1");
        assert_eq!(data.entitlements.len(), 1);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let mut v = valid_license();
        v.as_object_mut()
            .unwrap()
            .insert("bogus".into(), json!(true));
        assert!(parse(&v).is_err());
    }

    #[test]
    fn rejects_empty_entitlements() {
        let mut v = valid_license();
        v.as_object_mut()
            .unwrap()
            .insert("entitlements".into(), json!([]));
        assert!(parse(&v).is_err());
    }

    #[test]
    fn rejects_duplicate_product_entitlements() {
        let mut v = valid_license();
        let dup = json!({"product": "cursedelete", "edition": "community", "licenseType": "perpetual", "seats": 1});
        v["entitlements"].as_array_mut().unwrap().push(dup);
        assert!(parse(&v).is_err());
    }

    #[test]
    fn rejects_timestamp_without_offset() {
        let mut v = valid_license();
        v["issuedAt"] = json!("2026-01-01T00:00:00");
        assert!(parse(&v).is_err());
    }

    #[test]
    fn accepts_timestamp_with_explicit_positive_offset() {
        let mut v = valid_license();
        v["issuedAt"] = json!("2026-01-01T00:00:00+10:00");
        assert!(parse(&v).is_ok());
    }

    #[test]
    fn device_binding_and_activation_must_both_be_present_or_absent() {
        let mut v = valid_license();
        v["deviceBinding"] = json!({
            "scheme": "os-machine-id-sha256-v1",
            "deviceId": "A".repeat(64),
        });
        assert!(parse(&v).is_err());
    }

    #[test]
    fn rejects_metadata_with_nested_object() {
        let mut v = valid_license();
        v["metadata"] = json!({"nested": {"a": 1}});
        assert!(parse(&v).is_err());
    }

    #[test]
    fn rejects_metadata_key_not_lower_camel_case() {
        let mut v = valid_license();
        v["metadata"] = json!({"NotCamel": "x"});
        assert!(parse(&v).is_err());
    }

    #[test]
    fn accepts_valid_metadata() {
        let mut v = valid_license();
        v["metadata"] = json!({"purchaseOrder": "PO-1", "seatCount": 5, "trial": false});
        assert!(parse(&v).is_ok());
    }

    #[test]
    fn online_activation_requires_lease_fields() {
        let mut v = valid_license();
        v["deviceBinding"] =
            json!({"scheme": "os-machine-id-sha256-v1", "deviceId": "A".repeat(64)});
        v["activation"] = json!({
            "activationId": "act-1",
            "mode": "online",
            "activatedAt": "2026-01-01T00:00:00Z"
        });
        assert!(parse(&v).is_err());
    }

    #[test]
    fn offline_activation_does_not_require_lease_fields() {
        let mut v = valid_license();
        v["deviceBinding"] =
            json!({"scheme": "os-machine-id-sha256-v1", "deviceId": "A".repeat(64)});
        v["activation"] = json!({
            "activationId": "act-1",
            "mode": "offline",
            "activatedAt": "2026-01-01T00:00:00Z"
        });
        assert!(parse(&v).is_ok());
    }

    #[test]
    fn device_id_must_be_64_hex_chars() {
        assert!(is_valid_device_id(&"a".repeat(64)));
        assert!(!is_valid_device_id(&"a".repeat(63)));
        assert!(!is_valid_device_id(
            "not-hex-at-all-not-hex-at-all-not-hex-at-all-not-hex-at-all1234"
        ));
    }
}
