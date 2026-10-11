//! Resolve the effective licence from every installed candidate file.
//!
//! No licence at all is unregistered Community -- the normal, fully
//! working state, not an error. Candidates that fail validation are
//! reported (so `license status` can explain why) but never stop the
//! application. When several candidates are valid, the highest edition
//! wins, ties going to the earliest (highest-precedence) location.

use std::cell::OnceCell;
use std::path::PathBuf;

use time::OffsetDateTime;

use crate::claims::ActivationClaims;
use crate::edition::Edition;
use crate::validate::{LicensePolicy, Rejection, VerifiedLicense};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub source: PathBuf,
    /// The file's text, or why it could not be read.
    pub contents: Result<String, String>,
}

#[derive(Debug, Clone)]
pub struct ActiveLicense {
    pub source: PathBuf,
    pub license: VerifiedLicense,
    /// Present exactly when the edition requires activation.
    pub activation: Option<ActivationClaims>,
}

impl ActiveLicense {
    pub fn edition(&self) -> Edition {
        self.license.claims.edition
    }
}

#[derive(Debug, Clone)]
pub struct RejectedCandidate {
    pub source: PathBuf,
    pub reason: Rejection,
}

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub active: Option<ActiveLicense>,
    pub rejected: Vec<RejectedCandidate>,
}

impl Resolution {
    /// The edition whose features the application should enable.
    pub fn edition(&self) -> Edition {
        self.active
            .as_ref()
            .map_or(Edition::Community, ActiveLicense::edition)
    }

    /// True when a valid licence (including a registered Community
    /// licence) is in effect.
    pub fn is_registered(&self) -> bool {
        self.active.is_some()
    }
}

/// Local state, besides the licence files themselves, that affects
/// resolution.
pub struct LocalState<'a> {
    /// Every stored Business activation certificate (user and system
    /// scope). A Business licence is active if any one of them is valid
    /// for it on this device.
    pub activation_certificates: &'a [&'a str],
    /// Licence ids this installation knows to be revoked: reported by the
    /// server, or listed by an administrator.
    pub revoked_license_ids: &'a [String],
    /// Computed only if a Business licence actually needs it.
    pub device_id: &'a dyn Fn() -> String,
}

pub fn resolve(
    policy: &LicensePolicy,
    candidates: Vec<Candidate>,
    local: &LocalState<'_>,
    now: OffsetDateTime,
) -> Resolution {
    let device_id = OnceCell::new();
    let mut valid: Vec<ActiveLicense> = Vec::new();
    let mut rejected = Vec::new();

    for candidate in candidates {
        let outcome = candidate
            .contents
            .map_err(Rejection::Malformed)
            .and_then(|text| policy.verify_license(&text, now))
            .and_then(|license| {
                if local
                    .revoked_license_ids
                    .contains(&license.claims.license_id)
                {
                    return Err(Rejection::Revoked);
                }
                if !license.claims.edition.requires_activation() {
                    return Ok((license, None));
                }
                let mut first_error = Rejection::ActivationRequired;
                for (i, cert) in local.activation_certificates.iter().enumerate() {
                    let device = device_id.get_or_init(|| (local.device_id)());
                    match policy.verify_activation(cert, &license.claims, device, now) {
                        Ok(claims) => return Ok((license, Some(claims))),
                        Err(e) if i == 0 => first_error = e,
                        Err(_) => {}
                    }
                }
                Err(first_error)
            });
        match outcome {
            Ok((license, activation)) => valid.push(ActiveLicense {
                source: candidate.source,
                license,
                activation,
            }),
            Err(reason) => rejected.push(RejectedCandidate {
                source: candidate.source,
                reason,
            }),
        }
    }

    let mut active: Option<ActiveLicense> = None;
    for license in valid {
        if active
            .as_ref()
            .is_none_or(|best| license.edition() > best.edition())
        {
            active = Some(license);
        }
    }
    Resolution { active, rejected }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithm::SignatureAlgorithm;
    use crate::envelope::EnvelopeKind;
    use crate::keys::TrustedKeySet;
    use crate::testing::{activation_claims, license_claims, seal, TestSigner};
    use crate::validate::ProductIdentity;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-10-11 0:00 UTC);

    fn signer() -> TestSigner {
        TestSigner::new("alpha", SignatureAlgorithm::Ed25519, 1)
    }

    fn policy() -> LicensePolicy {
        LicensePolicy {
            product: ProductIdentity {
                product_id: "app".into(),
                major_version: 2,
            },
            trusted_keys: TrustedKeySet::new(vec![signer().trusted_key()]).unwrap(),
            allowed_algorithms: vec![SignatureAlgorithm::Ed25519],
            business_offline_limit: None,
        }
    }

    fn candidate(name: &str, edition: Edition) -> Candidate {
        Candidate {
            source: PathBuf::from(name),
            contents: Ok(seal(
                EnvelopeKind::License,
                &license_claims("app", 2, edition),
                &[&signer()],
            )),
        }
    }

    fn no_activation() -> LocalState<'static> {
        LocalState {
            activation_certificates: &[],
            revoked_license_ids: &[],
            device_id: &|| panic!("device id must not be needed"),
        }
    }

    fn business_cert(device: &str) -> String {
        seal(
            EnvelopeKind::Activation,
            &activation_claims("lic_test_business", "app", 2, device),
            &[&signer()],
        )
    }

    #[test]
    fn no_licence_is_unregistered_community() {
        let r = resolve(&policy(), vec![], &no_activation(), NOW);
        assert_eq!(r.edition(), Edition::Community);
        assert!(!r.is_registered());
        assert!(r.rejected.is_empty());
    }

    #[test]
    fn registered_community_is_distinguished_from_unregistered() {
        let r = resolve(
            &policy(),
            vec![candidate("c", Edition::Community)],
            &no_activation(),
            NOW,
        );
        assert_eq!(r.edition(), Edition::Community);
        assert!(r.is_registered());
    }

    #[test]
    fn invalid_or_unreadable_licences_fall_back_to_community() {
        let r = resolve(
            &policy(),
            vec![
                Candidate {
                    source: "bad".into(),
                    contents: Ok("{corrupt".into()),
                },
                Candidate {
                    source: "unreadable".into(),
                    contents: Err("permission denied".into()),
                },
            ],
            &no_activation(),
            NOW,
        );
        assert_eq!(r.edition(), Edition::Community);
        assert!(!r.is_registered());
        assert_eq!(r.rejected.len(), 2);
    }

    #[test]
    fn highest_valid_edition_wins_regardless_of_order() {
        let r = resolve(
            &policy(),
            vec![
                candidate("user", Edition::Community),
                candidate("admin", Edition::Enterprise),
                candidate("other", Edition::Education),
            ],
            &no_activation(),
            NOW,
        );
        assert_eq!(r.edition(), Edition::Enterprise);
        assert_eq!(r.active.unwrap().source, PathBuf::from("admin"));
    }

    #[test]
    fn education_and_enterprise_never_need_a_device() {
        let r = resolve(
            &policy(),
            vec![
                candidate("e", Edition::Education),
                candidate("x", Edition::Enterprise),
            ],
            &no_activation(),
            NOW,
        );
        assert!(r.rejected.is_empty());
    }

    #[test]
    fn business_without_activation_requires_activation() {
        let r = resolve(
            &policy(),
            vec![candidate("b", Edition::Business)],
            &no_activation(),
            NOW,
        );
        assert_eq!(r.edition(), Edition::Community);
        assert_eq!(r.rejected[0].reason, Rejection::ActivationRequired);
    }

    #[test]
    fn activated_business_resolves_to_business() {
        let cert = business_cert("DEV1");
        let r = resolve(
            &policy(),
            vec![candidate("b", Edition::Business)],
            &LocalState {
                activation_certificates: &[&cert],
                revoked_license_ids: &[],
                device_id: &|| "DEV1".to_string(),
            },
            NOW,
        );
        assert_eq!(r.edition(), Edition::Business);
        assert!(r.active.unwrap().activation.is_some());
    }

    #[test]
    fn any_matching_certificate_activates_business() {
        let other = business_cert("SOMEONE-ELSE");
        let mine = business_cert("DEV1");
        let r = resolve(
            &policy(),
            vec![candidate("b", Edition::Business)],
            &LocalState {
                activation_certificates: &[&other, &mine],
                revoked_license_ids: &[],
                device_id: &|| "DEV1".to_string(),
            },
            NOW,
        );
        assert_eq!(r.edition(), Edition::Business);
    }

    #[test]
    fn locally_known_revocation_rejects_an_otherwise_valid_licence() {
        let r = resolve(
            &policy(),
            vec![
                candidate("e", Edition::Enterprise),
                candidate("c", Edition::Community),
            ],
            &LocalState {
                revoked_license_ids: &["lic_test_enterprise".to_string()],
                ..no_activation()
            },
            NOW,
        );
        assert_eq!(r.edition(), Edition::Community);
        assert!(r.is_registered());
        assert_eq!(r.rejected[0].reason, Rejection::Revoked);
    }
}
