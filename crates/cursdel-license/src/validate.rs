//! Local, offline licence validation for one application build.
//!
//! The order is fixed: signature first (envelope + trusted key + allowed
//! algorithm), then claims, then product identity, then major version,
//! then entitlement dates. No claim is looked at before its signature
//! verifies, and a valid signature alone never establishes entitlement --
//! two products trusting the same key still reject each other's licences.

use time::{Duration, OffsetDateTime};

use crate::algorithm::SignatureAlgorithm;
use crate::claims::{self, ActivationClaims, ClaimsError, LicenseClaims, Term};
use crate::envelope::{self, EnvelopeError, EnvelopeKind, VerifiedSignature};
use crate::keys::TrustedKeySet;

/// Why a licence or activation certificate is not usable. Every variant
/// leaves the application on unregistered Community functionality; none
/// of them is a reason to stop the application.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("not a valid licence file: {0}")]
    Malformed(String),
    #[error("unsupported licence format '{0}'")]
    UnsupportedFormat(String),
    #[error("licence schema version {0} is not supported by this version of the application")]
    UnsupportedSchema(u64),
    #[error("licence is signed only with unsupported algorithm(s): {}", .0.join(", "))]
    UnsupportedSignatureAlgorithm(Vec<String>),
    #[error("licence is signed only with algorithm(s) this application does not permit: {}", .0.join(", "))]
    DisallowedSignatureAlgorithm(Vec<String>),
    #[error("licence is signed by key(s) this build does not trust: {}", .0.join(", "))]
    UnsupportedSigningKey(Vec<String>),
    #[error("licence signature is invalid (key '{0}'); the file may have been modified")]
    InvalidSignature(String),
    #[error("licence is for product '{found}', not '{expected}'")]
    WrongProduct { expected: String, found: String },
    #[error("licence is for major version {found}, but this is version {expected}")]
    WrongMajorVersion { expected: u32, found: u32 },
    #[error("licence expired at {0}")]
    Expired(OffsetDateTime),
    #[error("Business licence is not activated on this device")]
    ActivationRequired,
    #[error("activation certificate is not valid: {0}")]
    ActivationInvalid(String),
}

impl From<EnvelopeError> for Rejection {
    fn from(e: EnvelopeError) -> Self {
        match e {
            EnvelopeError::Malformed(m) => Rejection::Malformed(m),
            EnvelopeError::UnsupportedFormat(f) => Rejection::UnsupportedFormat(f),
            EnvelopeError::UnsupportedAlgorithm(a) => Rejection::UnsupportedSignatureAlgorithm(a),
            EnvelopeError::DisallowedAlgorithm(a) => Rejection::DisallowedSignatureAlgorithm(a),
            EnvelopeError::UntrustedKey(k) => Rejection::UnsupportedSigningKey(k),
            EnvelopeError::InvalidSignature(k) => Rejection::InvalidSignature(k),
        }
    }
}

impl From<ClaimsError> for Rejection {
    fn from(e: ClaimsError) -> Self {
        match e {
            ClaimsError::UnsupportedSchema(v) => Rejection::UnsupportedSchema(v),
            ClaimsError::Malformed(m) => Rejection::Malformed(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductIdentity {
    pub product_id: String,
    pub major_version: u32,
}

#[derive(Debug, Clone)]
pub struct VerifiedLicense {
    pub claims: LicenseClaims,
    pub signed_by: Vec<VerifiedSignature>,
}

/// Everything one application build needs to validate licences locally.
#[derive(Debug, Clone)]
pub struct LicensePolicy {
    pub product: ProductIdentity,
    pub trusted_keys: TrustedKeySet,
    pub allowed_algorithms: Vec<SignatureAlgorithm>,
    /// Business only: how long an activation certificate may be relied
    /// on without contacting the server, measured from its `issuedAt`.
    /// `None` permits indefinite offline operation after activation
    /// (revocation then only takes effect at the next successful renewal).
    pub business_offline_limit: Option<Duration>,
}

impl LicensePolicy {
    /// Full validation of a licence, including its term.
    pub fn verify_license(
        &self,
        text: &str,
        now: OffsetDateTime,
    ) -> Result<VerifiedLicense, Rejection> {
        let verified = self.verify_license_identity(text)?;
        if let Term::Subscription { expires_at } = verified.claims.term {
            if now >= expires_at {
                return Err(Rejection::Expired(expires_at));
            }
        }
        Ok(verified)
    }

    /// Signature, product and major version only -- not the term. Used to
    /// identify an expired subscription so it can be renewed; never use
    /// this to grant features.
    pub fn verify_license_identity(&self, text: &str) -> Result<VerifiedLicense, Rejection> {
        let opened = envelope::open(
            text,
            EnvelopeKind::License,
            &self.trusted_keys,
            &self.allowed_algorithms,
        )?;
        let claims = claims::parse_license(&opened.payload)?;
        self.check_identity(&claims.product_id, claims.major_version)?;
        Ok(VerifiedLicense {
            claims,
            signed_by: opened.verified_by,
        })
    }

    /// Validate a Business activation certificate against the licence it
    /// claims to activate and the current device. The licence's own term
    /// is checked separately (by [`Self::verify_license`]), so an old
    /// certificate can never extend an expired subscription.
    pub fn verify_activation(
        &self,
        text: &str,
        license: &LicenseClaims,
        device_id: &str,
        now: OffsetDateTime,
    ) -> Result<ActivationClaims, Rejection> {
        let invalid = |m: String| Rejection::ActivationInvalid(m);
        let opened = envelope::open(
            text,
            EnvelopeKind::Activation,
            &self.trusted_keys,
            &self.allowed_algorithms,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let cert = claims::parse_activation(&opened.payload).map_err(|e| invalid(e.to_string()))?;

        self.check_identity(&cert.product_id, cert.major_version)
            .map_err(|e| invalid(e.to_string()))?;
        if cert.license_id != license.license_id {
            return Err(invalid(format!(
                "it activates licence '{}', not '{}'",
                cert.license_id, license.license_id
            )));
        }
        if !cert.device_id.eq_ignore_ascii_case(device_id) {
            return Err(invalid("it was issued for a different device".into()));
        }
        if let Some(expires_at) = cert.expires_at {
            if now >= expires_at {
                return Err(invalid(format!(
                    "it expired at {expires_at}; run renew to refresh it"
                )));
            }
        }
        if let Some(limit) = self.business_offline_limit {
            if now >= cert.issued_at + limit {
                return Err(invalid(
                    "it has not been refreshed recently enough; run renew".into(),
                ));
            }
        }
        Ok(cert)
    }

    fn check_identity(&self, product_id: &str, major_version: u32) -> Result<(), Rejection> {
        if product_id != self.product.product_id {
            return Err(Rejection::WrongProduct {
                expected: self.product.product_id.clone(),
                found: product_id.to_string(),
            });
        }
        if major_version != self.product.major_version {
            return Err(Rejection::WrongMajorVersion {
                expected: self.product.major_version,
                found: major_version,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edition::Edition;
    use crate::testing::{activation_claims, license_claims, seal, TestSigner};
    use serde_json::json;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-10-11 0:00 UTC);
    const DEVICE: &str = "AB12";

    fn signer() -> TestSigner {
        TestSigner::new("repass-signing-alpha", SignatureAlgorithm::Ed25519, 1)
    }

    fn policy_trusting(signers: &[&TestSigner]) -> LicensePolicy {
        LicensePolicy {
            product: ProductIdentity {
                product_id: "app".into(),
                major_version: 2,
            },
            trusted_keys: TrustedKeySet::new(signers.iter().map(|s| s.trusted_key()).collect())
                .unwrap(),
            allowed_algorithms: SignatureAlgorithm::ALL.to_vec(),
            business_offline_limit: None,
        }
    }

    fn licence(edition: Edition) -> String {
        seal(
            EnvelopeKind::License,
            &license_claims("app", 2, edition),
            &[&signer()],
        )
    }

    #[test]
    fn valid_licence_for_every_edition() {
        let policy = policy_trusting(&[&signer()]);
        for edition in Edition::ALL {
            let v = policy.verify_license(&licence(edition), NOW).unwrap();
            assert_eq!(v.claims.edition, edition);
        }
    }

    #[test]
    fn perpetual_licence_never_expires() {
        let policy = policy_trusting(&[&signer()]);
        let far_future = datetime!(2199-01-01 0:00 UTC);
        assert!(policy
            .verify_license(&licence(Edition::Enterprise), far_future)
            .is_ok());
    }

    #[test]
    fn expired_subscription_is_rejected_but_still_identifiable() {
        let mut claims = license_claims("app", 2, Edition::Business);
        claims["perpetual"] = json!(false);
        claims["expiresAt"] = json!("2026-06-01T00:00:00Z");
        let text = seal(EnvelopeKind::License, &claims, &[&signer()]);
        let policy = policy_trusting(&[&signer()]);
        assert_eq!(
            policy.verify_license(&text, NOW).unwrap_err(),
            Rejection::Expired(datetime!(2026-06-01 0:00 UTC))
        );
        assert!(policy.verify_license_identity(&text).is_ok());
        assert!(policy
            .verify_license(&text, datetime!(2026-05-01 0:00 UTC))
            .is_ok());
    }

    #[test]
    fn wrong_product_and_wrong_major_are_rejected_even_with_a_shared_key() {
        let policy = policy_trusting(&[&signer()]);
        let other_product = seal(
            EnvelopeKind::License,
            &license_claims("another-app", 2, Edition::Enterprise),
            &[&signer()],
        );
        assert!(matches!(
            policy.verify_license(&other_product, NOW),
            Err(Rejection::WrongProduct { .. })
        ));
        let v3 = seal(
            EnvelopeKind::License,
            &license_claims("app", 3, Edition::Enterprise),
            &[&signer()],
        );
        assert_eq!(
            policy.verify_license(&v3, NOW).unwrap_err(),
            Rejection::WrongMajorVersion {
                expected: 2,
                found: 3
            }
        );
    }

    #[test]
    fn historical_key_remains_valid_only_where_still_trusted() {
        let alpha = signer();
        let beta = TestSigner::new("repass-signing-beta", SignatureAlgorithm::Ed25519, 2);
        let old_licence = licence(Edition::Enterprise);

        // A later v2 build that keeps alpha alongside beta still accepts it.
        assert!(policy_trusting(&[&alpha, &beta])
            .verify_license(&old_licence, NOW)
            .is_ok());
        // A build that trusts only beta does not.
        assert_eq!(
            policy_trusting(&[&beta])
                .verify_license(&old_licence, NOW)
                .unwrap_err(),
            Rejection::UnsupportedSigningKey(vec!["repass-signing-alpha".into()])
        );
    }

    #[test]
    fn licences_from_each_trusted_algorithm_are_accepted() {
        let ed = signer();
        let p256 = TestSigner::new("fips-p256", SignatureAlgorithm::EcdsaP256Sha256, 2);
        let pq = TestSigner::new("pq-mldsa65", SignatureAlgorithm::MlDsa65, 3);
        let policy = policy_trusting(&[&ed, &p256, &pq]);
        for s in [&ed, &p256, &pq] {
            let text = seal(
                EnvelopeKind::License,
                &license_claims("app", 2, Edition::Enterprise),
                &[s],
            );
            assert!(policy.verify_license(&text, NOW).is_ok());
        }
    }

    #[test]
    fn invalid_signature_is_reported() {
        // A Community licence's signature attached to Enterprise claims.
        let mut tampered: serde_json::Value =
            serde_json::from_str(&licence(Edition::Community)).unwrap();
        let enterprise: serde_json::Value =
            serde_json::from_str(&licence(Edition::Enterprise)).unwrap();
        tampered["payload"] = enterprise["payload"].clone();
        assert_eq!(
            policy_trusting(&[&signer()])
                .verify_license(&tampered.to_string(), NOW)
                .unwrap_err(),
            Rejection::InvalidSignature("repass-signing-alpha".into())
        );
    }

    fn business_claims() -> LicenseClaims {
        policy_trusting(&[&signer()])
            .verify_license(&licence(Edition::Business), NOW)
            .unwrap()
            .claims
    }

    fn cert(mutate: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v = activation_claims("lic_test_business", "app", 2, DEVICE);
        mutate(&mut v);
        seal(EnvelopeKind::Activation, &v, &[&signer()])
    }

    #[test]
    fn activation_certificate_for_this_device_is_accepted() {
        let policy = policy_trusting(&[&signer()]);
        let c = policy
            .verify_activation(&cert(|_| {}), &business_claims(), "ab12", NOW)
            .unwrap();
        assert_eq!(c.activation_id, "act_test_001");
    }

    #[test]
    fn activation_certificate_is_bound_to_device_licence_and_expiry() {
        let policy = policy_trusting(&[&signer()]);
        let lic = business_claims();
        let rejected = |text: String, device: &str| {
            matches!(
                policy.verify_activation(&text, &lic, device, NOW),
                Err(Rejection::ActivationInvalid(_))
            )
        };
        assert!(rejected(cert(|_| {}), "OTHER-DEVICE"));
        assert!(rejected(
            cert(|v| v["licenseId"] = json!("lic_other")),
            DEVICE
        ));
        assert!(rejected(cert(|v| v["majorVersion"] = json!(3)), DEVICE));
        assert!(rejected(
            cert(|v| v["expiresAt"] = json!("2026-02-01T00:00:00Z")),
            DEVICE
        ));
        // A licence envelope is not an activation certificate.
        assert!(rejected(licence(Edition::Business), DEVICE));
    }

    #[test]
    fn offline_limit_forces_periodic_renewal_when_configured() {
        let mut policy = policy_trusting(&[&signer()]);
        policy.business_offline_limit = Some(Duration::days(30));
        let lic = business_claims();
        assert!(policy
            .verify_activation(&cert(|_| {}), &lic, DEVICE, datetime!(2026-01-20 0:00 UTC))
            .is_ok());
        assert!(matches!(
            policy.verify_activation(&cert(|_| {}), &lic, DEVICE, NOW),
            Err(Rejection::ActivationInvalid(_))
        ));
    }
}
