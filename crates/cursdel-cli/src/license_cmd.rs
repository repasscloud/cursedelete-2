//! `cursdel license status|install|activate|deactivate|renew`.
//!
//! A licence problem never stops ordinary deletion: anything that fails
//! validation leaves CurseDelete on Community. Commands that change local
//! state validate everything the server returns before writing anything,
//! and a failed network call never removes or replaces an existing
//! licence or activation. Client secrets are never printed.

use std::path::Path;

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use cursdel_core::exit_code::ExitCode;
use cursdel_license::resolve::ActivationInput;
use cursdel_license::service::{
    ActivateRequest, ActivationRef, Product, RenewRequest, CODE_ACTIVATION_INACTIVE,
};
use cursdel_license::store::{generate_client_secret, ActivationCredentials};
use cursdel_license::{
    device, Edition, LicenseLocations, LicensePolicy, LicenseService, Resolution, ServiceError,
    Term, VerifiedLicense,
};
use cursdel_policy::Feature;

use crate::args::LicenseAction;

/// Everything a licence command depends on, injected so the commands can
/// be tested without real storage locations, devices, or a server.
pub struct Context {
    pub policy: LicensePolicy,
    pub locations: LicenseLocations,
    pub device_id: Box<dyn Fn() -> String>,
    pub now: OffsetDateTime,
}

impl Context {
    pub fn production() -> Self {
        Self {
            policy: cursdel_policy::license_policy(),
            locations: cursdel_policy::locations(),
            device_id: Box::new(|| device::current(cursdel_policy::DEVICE_NAMESPACE).device_id),
            now: OffsetDateTime::now_utc(),
        }
    }

    pub fn resolve(&self) -> Resolution {
        let certificate = self.locations.read_activation_certificate().ok().flatten();
        cursdel_license::resolve(
            &self.policy,
            self.locations.candidates(),
            &ActivationInput {
                certificate: certificate.as_deref(),
                device_id: &*self.device_id,
            },
            self.now,
        )
    }

    fn product(&self) -> Product<'_> {
        Product {
            product_id: &self.policy.product.product_id,
            major_version: self.policy.product.major_version,
        }
    }
}

/// The edition currently in effect on this machine. Never fails: any
/// problem resolves to Community.
pub fn current_edition() -> Edition {
    Context::production().resolve().edition()
}

pub fn run(action: LicenseAction) -> ExitCode {
    let ctx = Context::production();
    let online = |f: &dyn Fn(&dyn LicenseService) -> ExitCode| match cursdel_policy::service() {
        Ok(service) => f(service.as_ref()),
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::LicenseRequired
        }
    };
    match action {
        LicenseAction::Status { json } => status(&ctx, json),
        LicenseAction::Install { license_file } => install(&ctx, &license_file),
        LicenseAction::Activate { activation_key } => {
            online(&|s| activate(&ctx, s, &activation_key))
        }
        LicenseAction::Deactivate => online(&|s| deactivate(&ctx, s)),
        LicenseAction::Renew => online(&|s| renew(&ctx, s)),
    }
}

fn timestamp(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

fn describe_term(term: Term) -> String {
    match term {
        Term::Perpetual => "perpetual".to_string(),
        Term::Subscription { expires_at } => {
            format!("subscription, expires {}", timestamp(expires_at))
        }
    }
}

fn status(ctx: &Context, json: bool) -> ExitCode {
    let resolution = ctx.resolve();
    if json {
        println!("{}", status_json(ctx, &resolution));
        return ExitCode::Success;
    }

    let product = &ctx.policy.product;
    println!("CurseDelete License Status\n");
    match &resolution.active {
        None => {
            println!("Edition:      Community (unregistered)");
            println!(
                "Product:      {} v{}",
                product.product_id, product.major_version
            );
            println!();
            println!("No licence is in effect -- running with Community capabilities.");
            println!("Install a licence file with 'cursdel license install <file>', or");
            println!("activate online with 'cursdel license activate <key>'.");
        }
        Some(active) => {
            let c = &active.license.claims;
            let registered = if c.edition == Edition::Community {
                " (registered)"
            } else {
                ""
            };
            println!("Edition:      {}{registered}", c.edition);
            println!("Product:      {} v{}", c.product_id, c.major_version);
            println!("Licence:      {} ({})", c.license_id, describe_term(c.term));
            if let Some(org) = &c.organisation {
                println!("Organisation: {org}");
            }
            if let Some(name) = &c.customer_name {
                println!("Customer:     {name}");
            }
            println!("Customer ID:  {}", c.customer_id);
            if let Some(seats) = c.max_activations {
                println!("Seats:        {seats}");
            }
            if let Some(a) = &active.activation {
                let expiry = a.expires_at.map_or("no expiry".to_string(), |e| {
                    format!("expires {}", timestamp(e))
                });
                println!("Activation:   {} (this device, {expiry})", a.activation_id);
            }
            let signers: Vec<String> = active
                .license
                .signed_by
                .iter()
                .map(|s| format!("{} ({})", s.key_id, s.algorithm))
                .collect();
            println!("Signed by:    {}", signers.join(", "));
            println!("Source:       {}", active.source.display());
        }
    }
    println!();
    println!("Features:");
    for feature in Feature::ALL {
        let state = if feature.is_enabled(resolution.edition()) {
            "enabled"
        } else {
            "not included in this edition"
        };
        println!("  {:<22}{state}", feature.name());
    }
    if !resolution.rejected.is_empty() {
        println!();
        println!("Licence files not in use:");
        for r in &resolution.rejected {
            println!("  {}", r.source.display());
            println!("    {}", r.reason);
        }
    }
    ExitCode::Success
}

fn status_json(ctx: &Context, resolution: &Resolution) -> Value {
    let license = resolution.active.as_ref().map(|a| {
        let c = &a.license.claims;
        json!({
            "licenseId": c.license_id,
            "customerId": c.customer_id,
            "customerName": c.customer_name,
            "organisation": c.organisation,
            "perpetual": c.term == Term::Perpetual,
            "expiresAt": match c.term {
                Term::Perpetual => None,
                Term::Subscription { expires_at } => Some(timestamp(expires_at)),
            },
            "maxActivations": c.max_activations,
            "source": a.source.display().to_string(),
            "signedBy": a.license.signed_by.iter().map(|s| json!({
                "keyId": s.key_id,
                "algorithm": s.algorithm.id(),
            })).collect::<Vec<_>>(),
        })
    });
    let activation = resolution
        .active
        .as_ref()
        .and_then(|a| a.activation.as_ref())
        .map(|a| {
            json!({
                "activationId": a.activation_id,
                "expiresAt": a.expires_at.map(timestamp),
            })
        });
    let features: serde_json::Map<String, Value> = Feature::ALL
        .iter()
        .map(|f| {
            (
                f.name().to_string(),
                json!(f.is_enabled(resolution.edition())),
            )
        })
        .collect();
    json!({
        "edition": resolution.edition().id(),
        "registered": resolution.is_registered(),
        "product": {
            "productId": ctx.policy.product.product_id,
            "majorVersion": ctx.policy.product.major_version,
        },
        "license": license,
        "activation": activation,
        "features": features,
        "rejected": resolution.rejected.iter().map(|r| json!({
            "source": r.source.display().to_string(),
            "reason": r.reason.to_string(),
        })).collect::<Vec<_>>(),
    })
}

fn install(ctx: &Context, path: &Path) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: could not read {}: {e}", path.display());
            return ExitCode::CliUsageError;
        }
    };
    let verified = match ctx.policy.verify_license(&text, ctx.now) {
        Ok(v) => v,
        Err(reason) => {
            eprintln!("Error: licence not installed: {reason}");
            eprintln!("Nothing was changed.");
            return ExitCode::LicenseRequired;
        }
    };

    let previous = std::fs::read_to_string(ctx.locations.user_license_file())
        .ok()
        .and_then(|t| ctx.policy.verify_license(&t, ctx.now).ok());
    if let Err(e) = ctx.locations.write_user_license(&text) {
        eprintln!("Error: could not save licence: {e}");
        return ExitCode::UnexpectedFatal;
    }

    let c = &verified.claims;
    println!(
        "Installed {} licence {} for CurseDelete v{} ({}).",
        c.edition,
        c.license_id,
        c.major_version,
        describe_term(c.term)
    );
    if let Some(prev) = previous.filter(|p| p.claims.license_id != c.license_id) {
        println!(
            "Replaced the previously installed {} licence {}.",
            prev.claims.edition, prev.claims.license_id
        );
    }
    if c.edition.requires_activation() && ctx.resolve().edition() != Edition::Business {
        println!();
        println!("Business licences must also be activated on each device:");
        println!("  cursdel license activate <key>");
    }
    ExitCode::Success
}

fn report_service_error(action: &str, e: &ServiceError) {
    match e {
        ServiceError::NotConfigured => {
            eprintln!(
                "Error: {action} needs the licensing server, but none is configured.\n\
                 Set {} to its https:// URL, or install a licence file with\n\
                 'cursdel license install <file>'.",
                cursdel_policy::SERVER_URL_ENV
            );
        }
        ServiceError::Unavailable(m) => {
            eprintln!("Error: could not reach the licensing server: {m}");
        }
        other => eprintln!("Error: {action} failed: {other}"),
    }
}

fn activate(ctx: &Context, service: &dyn LicenseService, activation_key: &str) -> ExitCode {
    let device_id = (ctx.device_id)();
    let client_secret = generate_client_secret();
    let grant = match service.activate(&ActivateRequest {
        product: ctx.product(),
        activation_key,
        device_id: &device_id,
        client_secret: &client_secret,
    }) {
        Ok(g) => g,
        Err(e) => {
            report_service_error("activation", &e);
            eprintln!("Nothing was changed.");
            return ExitCode::LicenseRequired;
        }
    };

    let unusable = |what: &str, reason: &dyn std::fmt::Display| {
        eprintln!("Error: the licensing server returned {what} this build cannot use: {reason}");
        eprintln!("Nothing was changed.");
        ExitCode::LicenseRequired
    };
    let license = match ctx.policy.verify_license(&grant.license, ctx.now) {
        Ok(l) => l,
        Err(r) => return unusable("a licence", &r),
    };
    let c = &license.claims;

    let mut activation_id = None;
    if c.edition.requires_activation() {
        let Some(cert) = &grant.activation else {
            return unusable(
                "a Business licence",
                &"no activation certificate was included",
            );
        };
        let claims = match ctx.policy.verify_activation(cert, c, &device_id, ctx.now) {
            Ok(a) => a,
            Err(r) => return unusable("an activation certificate", &r),
        };
        let replaced = ctx
            .locations
            .read_credentials()
            .ok()
            .flatten()
            .filter(|old| old.activation_id != claims.activation_id);
        let credentials = ActivationCredentials {
            activation_id: claims.activation_id.clone(),
            client_secret,
        };
        if let Err(e) = ctx.locations.write_activation(cert, &credentials) {
            eprintln!("Error: could not save the activation: {e}");
            eprintln!(
                "The seat was allocated on the server; release activation {} from the customer portal if needed.",
                claims.activation_id
            );
            return ExitCode::UnexpectedFatal;
        }
        if let Some(old) = replaced {
            println!(
                "Note: this device's previous activation {} was replaced locally; release it from\n\
                 the customer portal if it still holds a seat.",
                old.activation_id
            );
        }
        activation_id = Some(claims.activation_id);
    }

    if let Err(e) = ctx.locations.write_user_license(&grant.license) {
        eprintln!("Error: could not save licence: {e}");
        return ExitCode::UnexpectedFatal;
    }
    println!(
        "Activated {} licence {} for CurseDelete v{} ({}).",
        c.edition,
        c.license_id,
        c.major_version,
        describe_term(c.term)
    );
    if let Some(id) = activation_id {
        println!("This device is activation {id}.");
    }
    ExitCode::Success
}

fn deactivate(ctx: &Context, service: &dyn LicenseService) -> ExitCode {
    let credentials = match ctx.locations.read_credentials() {
        Ok(Some(c)) => c,
        Ok(None) => {
            println!("No Business activation is stored on this device.");
            return ExitCode::Success;
        }
        Err(e) => {
            eprintln!("Error: could not read activation credentials: {e}");
            return ExitCode::UnexpectedFatal;
        }
    };
    let device_id = (ctx.device_id)();
    let activation = ActivationRef {
        activation_id: &credentials.activation_id,
        device_id: &device_id,
        client_secret: &credentials.client_secret,
    };
    let already_released = match service.deactivate(ctx.product(), &activation) {
        Ok(()) => false,
        Err(ServiceError::Rejected { code, .. }) if code == CODE_ACTIVATION_INACTIVE => true,
        Err(e) => {
            report_service_error("deactivation", &e);
            eprintln!(
                "Activation {} was NOT released and is still in place on this device.",
                credentials.activation_id
            );
            return ExitCode::LicenseRequired;
        }
    };
    if let Err(e) = ctx.locations.remove_activation() {
        eprintln!(
            "Warning: the seat was released on the server, but the local activation could not be removed: {e}"
        );
        return ExitCode::UnexpectedFatal;
    }
    if already_released {
        println!(
            "Activation {} had already been released on the server; removed it from this device.",
            credentials.activation_id
        );
    } else {
        println!(
            "Activation {} released; its seat is free for another device.",
            credentials.activation_id
        );
    }
    println!("The licence file stays installed; activate again to use Business features here.");
    ExitCode::Success
}

/// The licence to renew: the one in effect, or else any installed licence
/// for this product and major version whose signature verifies -- an
/// expired subscription is exactly what `renew` exists for.
fn renewable(ctx: &Context, resolution: Resolution) -> Option<VerifiedLicense> {
    resolution.active.map(|a| a.license).or_else(|| {
        ctx.locations
            .candidates()
            .into_iter()
            .filter_map(|c| c.contents.ok())
            .find_map(|text| ctx.policy.verify_license_identity(&text).ok())
    })
}

fn renew(ctx: &Context, service: &dyn LicenseService) -> ExitCode {
    let Some(current) = renewable(ctx, ctx.resolve()) else {
        eprintln!(
            "Error: no CurseDelete v{} licence is installed to renew.",
            ctx.policy.product.major_version
        );
        eprintln!("Install one with 'cursdel license install <file>' or 'cursdel license activate <key>'.");
        return ExitCode::LicenseRequired;
    };
    let current = current.claims;
    let credentials = if current.edition.requires_activation() {
        ctx.locations.read_credentials().ok().flatten()
    } else {
        None
    };
    let device_id = credentials.as_ref().map(|_| (ctx.device_id)());
    let activation = credentials
        .as_ref()
        .zip(device_id.as_deref())
        .map(|(c, device_id)| ActivationRef {
            activation_id: &c.activation_id,
            device_id,
            client_secret: &c.client_secret,
        });

    let keep = || {
        eprintln!("The installed licence was left unchanged.");
        ExitCode::LicenseRequired
    };
    let grant = match service.renew(&RenewRequest {
        product: ctx.product(),
        license_id: &current.license_id,
        activation,
    }) {
        Ok(g) => g,
        Err(e) => {
            report_service_error("renewal", &e);
            return keep();
        }
    };
    let renewed = match ctx.policy.verify_license(&grant.license, ctx.now) {
        Ok(l) => l.claims,
        Err(r) => {
            eprintln!("Error: the licensing server returned a licence this build cannot use: {r}");
            return keep();
        }
    };

    if let (Some(cert), Some(creds), Some(device_id)) =
        (&grant.activation, &credentials, &device_id)
    {
        let claims = match ctx
            .policy
            .verify_activation(cert, &renewed, device_id, ctx.now)
        {
            Ok(a) => a,
            Err(r) => {
                eprintln!("Error: the renewed activation certificate is not usable: {r}");
                return keep();
            }
        };
        let creds = ActivationCredentials {
            activation_id: claims.activation_id,
            client_secret: creds.client_secret.clone(),
        };
        if let Err(e) = ctx.locations.write_activation(cert, &creds) {
            eprintln!("Error: could not save the renewed activation: {e}");
            return ExitCode::UnexpectedFatal;
        }
    }
    if let Err(e) = ctx.locations.write_user_license(&grant.license) {
        eprintln!("Error: could not save the renewed licence: {e}");
        return ExitCode::UnexpectedFatal;
    }
    println!(
        "Renewed {} licence {} ({}).",
        renewed.edition,
        renewed.license_id,
        describe_term(renewed.term)
    );
    if renewed.edition.requires_activation() && ctx.resolve().edition() != Edition::Business {
        println!("This device still needs activation: cursdel license activate <key>");
    }
    ExitCode::Success
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use cursdel_license::envelope::EnvelopeKind;
    use cursdel_license::service::Grant;
    use cursdel_license::testing::{activation_claims, license_claims, seal, TestSigner};
    use cursdel_license::{ProductIdentity, SignatureAlgorithm, TrustedKeySet};
    use serde_json::json;
    use time::macros::datetime;

    use super::*;

    const DEVICE: &str = "DEVICE-1";

    fn signer() -> TestSigner {
        TestSigner::new("test-alpha", SignatureAlgorithm::Ed25519, 1)
    }

    fn context(dir: &Path) -> Context {
        Context {
            policy: LicensePolicy {
                product: ProductIdentity {
                    product_id: "cursedelete".into(),
                    major_version: 2,
                },
                trusted_keys: TrustedKeySet::new(vec![signer().trusted_key()]).unwrap(),
                allowed_algorithms: SignatureAlgorithm::ALL.to_vec(),
                business_offline_limit: None,
            },
            locations: LicenseLocations {
                override_file: None,
                user_dir: dir.join("user"),
                admin_dir: Some(dir.join("admin")),
            },
            device_id: Box::new(|| DEVICE.to_string()),
            now: datetime!(2026-10-11 0:00 UTC),
        }
    }

    fn licence_with(edition: Edition, mutate: impl FnOnce(&mut Value)) -> String {
        let mut claims = license_claims("cursedelete", 2, edition);
        mutate(&mut claims);
        seal(EnvelopeKind::License, &claims, &[&signer()])
    }

    fn licence(edition: Edition) -> String {
        licence_with(edition, |_| {})
    }

    fn certificate(activation_id: &str) -> String {
        let mut claims = activation_claims("lic_test_business", "cursedelete", 2, DEVICE);
        claims["activationId"] = json!(activation_id);
        seal(EnvelopeKind::Activation, &claims, &[&signer()])
    }

    /// Records every call; answers with a fixed result.
    struct FakeService {
        grant: Result<Grant, ServiceError>,
        deactivate: Result<(), ServiceError>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeService {
        fn granting(license: String, activation: Option<String>) -> Self {
            Self {
                grant: Ok(Grant {
                    license,
                    activation,
                }),
                deactivate: Ok(()),
                calls: RefCell::default(),
            }
        }

        fn failing(error: ServiceError) -> Self {
            Self {
                grant: Err(error.clone()),
                deactivate: Err(error),
                calls: RefCell::default(),
            }
        }
    }

    impl LicenseService for FakeService {
        fn activate(&self, r: &ActivateRequest<'_>) -> Result<Grant, ServiceError> {
            assert_eq!(r.product.product_id, "cursedelete");
            assert_eq!(r.device_id, DEVICE);
            self.calls
                .borrow_mut()
                .push(format!("activate:{}", r.activation_key));
            self.grant.clone()
        }
        fn renew(&self, r: &RenewRequest<'_>) -> Result<Grant, ServiceError> {
            self.calls.borrow_mut().push(format!(
                "renew:{}:{}",
                r.license_id,
                r.activation.map_or("-", |a| a.activation_id)
            ));
            self.grant.clone()
        }
        fn deactivate(&self, _: Product<'_>, a: &ActivationRef<'_>) -> Result<(), ServiceError> {
            self.calls
                .borrow_mut()
                .push(format!("deactivate:{}", a.activation_id));
            self.deactivate.clone()
        }
    }

    fn file(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn nothing_installed_is_unregistered_community() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let r = ctx.resolve();
        assert_eq!(r.edition(), Edition::Community);
        assert!(!r.is_registered());
        assert_eq!(status(&ctx, false), ExitCode::Success);
        let j = status_json(&ctx, &r);
        assert_eq!(j["edition"], "community");
        assert_eq!(j["registered"], false);
        assert_eq!(j["features"]["--close-remote-locks"], false);
    }

    #[test]
    fn installing_an_enterprise_licence_enables_gated_features_offline() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let path = file(dir.path(), "ent.license", &licence(Edition::Enterprise));
        assert_eq!(install(&ctx, &path), ExitCode::Success);
        let r = ctx.resolve();
        assert_eq!(r.edition(), Edition::Enterprise);
        assert!(Feature::CloseRemoteLocks.is_enabled(r.edition()));
        assert_eq!(
            status_json(&ctx, &r)["features"]["--close-remote-locks"],
            true
        );
    }

    #[test]
    fn education_and_registered_community_install_offline() {
        for edition in [Edition::Education, Edition::Community] {
            let dir = tempfile::tempdir().unwrap();
            let ctx = context(dir.path());
            let path = file(dir.path(), "l.license", &licence(edition));
            assert_eq!(install(&ctx, &path), ExitCode::Success);
            let r = ctx.resolve();
            assert_eq!(r.edition(), edition);
            assert!(r.is_registered());
            assert!(!Feature::CloseRemoteLocks.is_enabled(r.edition()));
        }
    }

    #[test]
    fn admin_directory_licence_is_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        std::fs::create_dir_all(dir.path().join("admin")).unwrap();
        file(
            &dir.path().join("admin"),
            "site.license",
            &licence(Edition::Enterprise),
        );
        assert_eq!(ctx.resolve().edition(), Edition::Enterprise);
    }

    #[test]
    fn invalid_licences_are_never_installed_and_never_replace_a_valid_one() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let good = file(dir.path(), "good.license", &licence(Edition::Enterprise));
        assert_eq!(install(&ctx, &good), ExitCode::Success);

        let stranger = TestSigner::new("stranger", SignatureAlgorithm::Ed25519, 9);
        let untrusted = seal(
            EnvelopeKind::License,
            &license_claims("cursedelete", 2, Edition::Enterprise),
            &[&stranger],
        );
        let wrong_product = licence_with(Edition::Enterprise, |c| c["productId"] = json!("other"));
        let wrong_major = licence_with(Edition::Enterprise, |c| c["majorVersion"] = json!(3));
        let expired = licence_with(Edition::Business, |c| {
            c["perpetual"] = json!(false);
            c["expiresAt"] = json!("2026-02-01T00:00:00Z");
        });
        for (name, text) in [
            ("corrupt", "{ not json".to_string()),
            ("untrusted", untrusted),
            ("product", wrong_product),
            ("major", wrong_major),
            ("expired", expired),
        ] {
            let path = file(dir.path(), name, &text);
            assert_eq!(install(&ctx, &path), ExitCode::LicenseRequired, "{name}");
            assert_eq!(ctx.resolve().edition(), Edition::Enterprise, "{name}");
        }
        assert_eq!(
            install(&ctx, &dir.path().join("missing")),
            ExitCode::CliUsageError
        );
    }

    #[test]
    fn business_licence_file_alone_needs_activation() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let path = file(dir.path(), "b.license", &licence(Edition::Business));
        assert_eq!(install(&ctx, &path), ExitCode::Success);
        let r = ctx.resolve();
        assert_eq!(r.edition(), Edition::Community);
        assert_eq!(
            r.rejected[0].reason,
            cursdel_license::Rejection::ActivationRequired
        );
    }

    #[test]
    fn business_activation_stores_licence_certificate_and_secret() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let svc = FakeService::granting(licence(Edition::Business), Some(certificate("act_1")));
        assert_eq!(activate(&ctx, &svc, "KEY-1"), ExitCode::Success);
        assert_eq!(svc.calls.borrow().as_slice(), ["activate:KEY-1"]);
        assert_eq!(ctx.resolve().edition(), Edition::Business);
        let creds = ctx.locations.read_credentials().unwrap().unwrap();
        assert_eq!(creds.activation_id, "act_1");
        assert!(!creds.client_secret.is_empty());
    }

    #[test]
    fn enterprise_online_provisioning_stores_no_device_state() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let svc = FakeService::granting(licence(Edition::Enterprise), None);
        assert_eq!(activate(&ctx, &svc, "KEY"), ExitCode::Success);
        assert_eq!(ctx.resolve().edition(), Edition::Enterprise);
        assert_eq!(ctx.locations.read_credentials().unwrap(), None);
        assert_eq!(ctx.locations.read_activation_certificate().unwrap(), None);
    }

    #[test]
    fn activation_failures_change_nothing() {
        let stranger = TestSigner::new("stranger", SignatureAlgorithm::Ed25519, 9);
        let foreign_cert = seal(
            EnvelopeKind::Activation,
            &activation_claims("lic_test_business", "cursedelete", 2, "SOME-OTHER-DEVICE"),
            &[&signer()],
        );
        let untrusted = seal(
            EnvelopeKind::License,
            &license_claims("cursedelete", 2, Edition::Enterprise),
            &[&stranger],
        );
        let cases = [
            FakeService::failing(ServiceError::Unavailable("timeout".into())),
            FakeService::failing(ServiceError::NotConfigured),
            FakeService::failing(ServiceError::Rejected {
                code: "seat_limit".into(),
                message: "all 3 seats are in use".into(),
            }),
            FakeService::granting(untrusted, None),
            FakeService::granting(licence(Edition::Business), None),
            FakeService::granting(licence(Edition::Business), Some(foreign_cert)),
        ];
        for svc in cases {
            let dir = tempfile::tempdir().unwrap();
            let ctx = context(dir.path());
            assert_eq!(activate(&ctx, &svc, "KEY"), ExitCode::LicenseRequired);
            assert!(!ctx.locations.user_license_file().exists());
            assert_eq!(ctx.locations.read_credentials().unwrap(), None);
        }
    }

    fn activated(dir: &Path) -> Context {
        let ctx = context(dir);
        let svc = FakeService::granting(licence(Edition::Business), Some(certificate("act_1")));
        assert_eq!(activate(&ctx, &svc, "KEY"), ExitCode::Success);
        ctx
    }

    #[test]
    fn deactivation_releases_the_seat_and_keeps_the_licence_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = activated(dir.path());
        let svc = FakeService::granting(String::new(), None);
        assert_eq!(deactivate(&ctx, &svc), ExitCode::Success);
        assert_eq!(svc.calls.borrow().as_slice(), ["deactivate:act_1"]);
        assert_eq!(ctx.locations.read_credentials().unwrap(), None);
        assert!(ctx.locations.user_license_file().exists());
        assert_eq!(ctx.resolve().edition(), Edition::Community);
    }

    #[test]
    fn failed_deactivation_keeps_the_local_activation() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = activated(dir.path());
        let svc = FakeService::failing(ServiceError::Unavailable("offline".into()));
        assert_eq!(deactivate(&ctx, &svc), ExitCode::LicenseRequired);
        assert!(ctx.locations.read_credentials().unwrap().is_some());
        assert_eq!(ctx.resolve().edition(), Edition::Business);
    }

    #[test]
    fn portal_released_activation_is_cleaned_up_locally() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = activated(dir.path());
        let svc = FakeService::failing(ServiceError::Rejected {
            code: CODE_ACTIVATION_INACTIVE.into(),
            message: "released via portal".into(),
        });
        assert_eq!(deactivate(&ctx, &svc), ExitCode::Success);
        assert_eq!(ctx.locations.read_credentials().unwrap(), None);
    }

    #[test]
    fn deactivate_without_activation_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let svc = FakeService::failing(ServiceError::NotConfigured);
        assert_eq!(deactivate(&ctx, &svc), ExitCode::Success);
        assert!(svc.calls.borrow().is_empty());
    }

    fn subscription(expires_at: &str) -> String {
        licence_with(Edition::Business, |c| {
            c["perpetual"] = json!(false);
            c["expiresAt"] = json!(expires_at);
        })
    }

    #[test]
    fn expired_subscription_reverts_to_community_and_renewal_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let svc = FakeService::granting(
            subscription("2026-12-01T00:00:00Z"),
            Some(certificate("act_1")),
        );
        assert_eq!(activate(&ctx, &svc, "KEY"), ExitCode::Success);

        let later = Context {
            now: datetime!(2027-01-15 0:00 UTC),
            ..context(dir.path())
        };
        assert_eq!(later.resolve().edition(), Edition::Community);

        let renewal = FakeService::granting(subscription("2028-01-01T00:00:00Z"), None);
        assert_eq!(renew(&later, &renewal), ExitCode::Success);
        assert_eq!(
            renewal.calls.borrow().as_slice(),
            ["renew:lic_test_business:act_1"]
        );
        assert_eq!(later.resolve().edition(), Edition::Business);
    }

    #[test]
    fn failed_renewal_keeps_the_existing_licence() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let path = file(dir.path(), "e.license", &licence(Edition::Enterprise));
        assert_eq!(install(&ctx, &path), ExitCode::Success);
        let before = std::fs::read_to_string(ctx.locations.user_license_file()).unwrap();

        let wrong_major = licence_with(Edition::Enterprise, |c| c["majorVersion"] = json!(3));
        for svc in [
            FakeService::failing(ServiceError::Unavailable("offline".into())),
            FakeService::failing(ServiceError::Revoked("refunded".into())),
            FakeService::granting(wrong_major, None),
        ] {
            assert_eq!(renew(&ctx, &svc), ExitCode::LicenseRequired);
            assert_eq!(
                std::fs::read_to_string(ctx.locations.user_license_file()).unwrap(),
                before
            );
            assert_eq!(ctx.resolve().edition(), Edition::Enterprise);
        }
    }

    #[test]
    fn renew_without_any_licence_is_refused_without_contacting_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let svc = FakeService::failing(ServiceError::NotConfigured);
        assert_eq!(renew(&ctx, &svc), ExitCode::LicenseRequired);
        assert!(svc.calls.borrow().is_empty());
    }
}
