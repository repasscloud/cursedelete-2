//! The signed claims inside a licence and inside a Business activation
//! certificate (schema version 1).
//!
//! Parsing is strict: unknown fields are rejected, so a future claim that
//! *restricts* a licence can never be silently ignored by an older build
//! -- adding a field means bumping `schemaVersion`, which older builds
//! report as unsupported. The edition rules that hold for every product
//! are enforced here too: Community, Education and Enterprise are always
//! perpetual, and only Business carries an activation allowance.

use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::edition::Edition;

pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimsError {
    #[error("unsupported schema version {0}")]
    UnsupportedSchema(u64),
    #[error("{0}")]
    Malformed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Term {
    Perpetual,
    Subscription { expires_at: OffsetDateTime },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseClaims {
    pub license_id: String,
    pub customer_id: String,
    pub product_id: String,
    pub major_version: u32,
    pub edition: Edition,
    pub organisation: Option<String>,
    pub customer_name: Option<String>,
    pub issued_at: OffsetDateTime,
    pub term: Term,
    /// Business only: simultaneously active devices. Enforced by the
    /// licensing server at activation time; informational here.
    pub max_activations: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationClaims {
    pub activation_id: String,
    pub license_id: String,
    pub product_id: String,
    pub major_version: u32,
    pub device_id: String,
    pub issued_at: OffsetDateTime,
    /// `None`: the certificate itself never expires (the licence's own
    /// term still applies).
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLicense {
    #[allow(dead_code)] // checked by `checked_value`
    schema_version: u64,
    license_id: String,
    customer_id: String,
    product_id: String,
    major_version: u32,
    edition: String,
    #[serde(default)]
    organisation: Option<String>,
    #[serde(default)]
    customer_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    issued_at: OffsetDateTime,
    perpetual: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
    #[serde(default)]
    max_activations: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawActivation {
    #[allow(dead_code)] // checked by `checked_value`
    schema_version: u64,
    activation_id: String,
    license_id: String,
    product_id: String,
    major_version: u32,
    device_id: String,
    #[serde(with = "time::serde::rfc3339")]
    issued_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

/// Check `schemaVersion` before the strict parse, so a newer schema is
/// reported as such rather than as an "unknown field".
fn checked_value(payload: &[u8]) -> Result<Value, ClaimsError> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|e| ClaimsError::Malformed(format!("claims are not valid JSON: {e}")))?;
    match value.get("schemaVersion").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => Ok(value),
        Some(other) => Err(ClaimsError::UnsupportedSchema(other)),
        None => Err(ClaimsError::Malformed(
            "claims are missing a numeric 'schemaVersion'".into(),
        )),
    }
}

fn malformed(e: serde_json::Error) -> ClaimsError {
    ClaimsError::Malformed(format!("invalid claims: {e}"))
}

fn non_empty(name: &str, value: String) -> Result<String, ClaimsError> {
    if value.trim().is_empty() {
        Err(ClaimsError::Malformed(format!(
            "'{name}' must not be empty"
        )))
    } else {
        Ok(value)
    }
}

fn optional_non_empty(name: &str, value: Option<String>) -> Result<Option<String>, ClaimsError> {
    value.map(|v| non_empty(name, v)).transpose()
}

pub fn parse_license(payload: &[u8]) -> Result<LicenseClaims, ClaimsError> {
    let raw: RawLicense = serde_json::from_value(checked_value(payload)?).map_err(malformed)?;

    let edition = Edition::from_id(&raw.edition)
        .ok_or_else(|| ClaimsError::Malformed(format!("unknown edition '{}'", raw.edition)))?;

    let term = match (raw.perpetual, raw.expires_at) {
        (true, None) => Term::Perpetual,
        (false, Some(expires_at)) if expires_at > raw.issued_at => {
            Term::Subscription { expires_at }
        }
        (false, Some(_)) => {
            return Err(ClaimsError::Malformed(
                "'expiresAt' must be after 'issuedAt'".into(),
            ))
        }
        (true, Some(_)) => {
            return Err(ClaimsError::Malformed(
                "a perpetual licence must not have 'expiresAt'".into(),
            ))
        }
        (false, None) => {
            return Err(ClaimsError::Malformed(
                "a non-perpetual licence must have 'expiresAt'".into(),
            ))
        }
    };

    if edition != Edition::Business && term != Term::Perpetual {
        return Err(ClaimsError::Malformed(format!(
            "{edition} licences are always perpetual"
        )));
    }
    match (edition, raw.max_activations) {
        (Edition::Business, Some(n)) if n >= 1 => {}
        (Edition::Business, _) => {
            return Err(ClaimsError::Malformed(
                "Business licences must have 'maxActivations' of at least 1".into(),
            ))
        }
        (_, Some(_)) => {
            return Err(ClaimsError::Malformed(format!(
                "{edition} licences have no activation limit; 'maxActivations' is not allowed"
            )))
        }
        (_, None) => {}
    }

    Ok(LicenseClaims {
        license_id: non_empty("licenseId", raw.license_id)?,
        customer_id: non_empty("customerId", raw.customer_id)?,
        product_id: non_empty("productId", raw.product_id)?,
        major_version: raw.major_version,
        edition,
        organisation: optional_non_empty("organisation", raw.organisation)?,
        customer_name: optional_non_empty("customerName", raw.customer_name)?,
        issued_at: raw.issued_at,
        term,
        max_activations: raw.max_activations,
    })
}

pub fn parse_activation(payload: &[u8]) -> Result<ActivationClaims, ClaimsError> {
    let raw: RawActivation = serde_json::from_value(checked_value(payload)?).map_err(malformed)?;
    if raw.expires_at.is_some_and(|e| e <= raw.issued_at) {
        return Err(ClaimsError::Malformed(
            "'expiresAt' must be after 'issuedAt'".into(),
        ));
    }
    Ok(ActivationClaims {
        activation_id: non_empty("activationId", raw.activation_id)?,
        license_id: non_empty("licenseId", raw.license_id)?,
        product_id: non_empty("productId", raw.product_id)?,
        major_version: raw.major_version,
        device_id: non_empty("deviceId", raw.device_id)?,
        issued_at: raw.issued_at,
        expires_at: raw.expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{activation_claims, license_claims};
    use serde_json::json;

    fn parse(v: &Value) -> Result<LicenseClaims, ClaimsError> {
        parse_license(v.to_string().as_bytes())
    }

    #[test]
    fn parses_each_edition_with_its_default_shape() {
        for edition in Edition::ALL {
            let claims = parse(&license_claims("app", 2, edition)).unwrap();
            assert_eq!(claims.edition, edition);
            assert_eq!(claims.term, Term::Perpetual);
        }
    }

    #[test]
    fn business_subscription_parses() {
        let mut v = license_claims("app", 2, Edition::Business);
        v["perpetual"] = json!(false);
        v["expiresAt"] = json!("2027-01-01T00:00:00Z");
        assert!(matches!(parse(&v).unwrap().term, Term::Subscription { .. }));
    }

    #[test]
    fn non_business_editions_cannot_be_subscriptions() {
        for edition in [Edition::Community, Edition::Education, Edition::Enterprise] {
            let mut v = license_claims("app", 2, edition);
            v["perpetual"] = json!(false);
            v["expiresAt"] = json!("2027-01-01T00:00:00Z");
            assert!(parse(&v).is_err(), "{edition} must be perpetual");
        }
    }

    #[test]
    fn perpetual_and_expiry_are_mutually_exclusive() {
        let mut v = license_claims("app", 2, Edition::Business);
        v["expiresAt"] = json!("2027-01-01T00:00:00Z");
        assert!(parse(&v).is_err());
        v["perpetual"] = json!(false);
        v["expiresAt"] = Value::Null;
        assert!(parse(&v).is_err());
    }

    #[test]
    fn max_activations_is_business_only() {
        let mut business = license_claims("app", 2, Edition::Business);
        business["maxActivations"] = Value::Null;
        assert!(parse(&business).is_err());
        business["maxActivations"] = json!(0);
        assert!(parse(&business).is_err());

        let mut enterprise = license_claims("app", 2, Edition::Enterprise);
        enterprise["maxActivations"] = json!(5);
        assert!(parse(&enterprise).is_err());
    }

    #[test]
    fn unknown_fields_and_editions_are_rejected() {
        let mut v = license_claims("app", 2, Edition::Community);
        v["unlimitedEverything"] = json!(true);
        assert!(parse(&v).is_err());

        let mut v = license_claims("app", 2, Edition::Community);
        v["edition"] = json!("platinum");
        assert!(parse(&v).is_err());
    }

    #[test]
    fn newer_schema_is_reported_as_unsupported_not_malformed() {
        let mut v = license_claims("app", 2, Edition::Community);
        v["schemaVersion"] = json!(2);
        v["someNewField"] = json!("x");
        assert_eq!(parse(&v).unwrap_err(), ClaimsError::UnsupportedSchema(2));
    }

    #[test]
    fn timestamps_require_an_explicit_offset() {
        let mut v = license_claims("app", 2, Edition::Community);
        v["issuedAt"] = json!("2026-01-01T00:00:00");
        assert!(parse(&v).is_err());
    }

    #[test]
    fn empty_identifiers_are_rejected() {
        let mut v = license_claims("app", 2, Edition::Community);
        v["licenseId"] = json!(" ");
        assert!(parse(&v).is_err());
    }

    #[test]
    fn activation_claims_parse_and_validate_expiry_order() {
        let v = activation_claims("lic", "app", 2, "DEVICE");
        assert!(parse_activation(v.to_string().as_bytes()).is_ok());

        let mut v = activation_claims("lic", "app", 2, "DEVICE");
        v["expiresAt"] = json!("2020-01-01T00:00:00Z");
        assert!(parse_activation(v.to_string().as_bytes()).is_err());
    }
}
