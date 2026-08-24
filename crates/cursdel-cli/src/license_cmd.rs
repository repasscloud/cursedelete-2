//! `cursdel license status|activate|import|deactivate|refresh`.
//!
//! Every command here fails closed to a clear message rather than
//! crashing: a licence problem is always reported as "not activated"
//! with an explanation, never a panic (`LICENSING-INTEGRATION.md` §9).
//! Activation bearer tokens are never printed.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use cursdel_core::exit_code::ExitCode;
use cursdel_license::client::{server_base_url, ClientError, LicenseServerClient};
use cursdel_license::store::{self, ActivationCredentials, LicensePaths};
use cursdel_license::{device, ProductEntitlement, Secret, VerifiedLicense};
use cursdel_policy::Capabilities;

use crate::args::LicenseAction;

pub fn run(action: LicenseAction) -> ExitCode {
    match action {
        LicenseAction::Status => status(),
        LicenseAction::Activate {
            license_id,
            activation_code,
            offline,
            output,
        } => activate(&license_id, &activation_code, offline, output),
        LicenseAction::Import { license_file } => import(&license_file),
        LicenseAction::Deactivate => deactivate(),
        LicenseAction::Refresh => refresh(),
        LicenseAction::Enroll {
            deployment_key,
            deployment_key_env,
            deployment_key_file,
            deployment_key_stdin,
        } => enroll(
            deployment_key,
            deployment_key_env,
            deployment_key_file,
            deployment_key_stdin,
        ),
    }
}

/// Which storage scope an on-disk licence/activation pair was found in.
/// Deployment Key enrollment always writes [`Scope::Machine`]; manual
/// `activate`/`import` always write [`Scope::User`]. `status`, `refresh`,
/// and `deactivate` need to know which one is actually present so they
/// operate on the right files without requiring the caller to remember
/// how this machine was set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Machine,
    User,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Scope::Machine => "machine-wide (Deployment Key enrollment)",
            Scope::User => "user",
        }
    }
}

/// Resolves which storage scope has activation credentials present,
/// preferring machine-wide over user scope. This is a narrow, local
/// version of the full machine -> user -> Community precedence that
/// belongs to the separate machine-wide storage/resolution work; it only
/// needs to decide which single `LicensePaths` the `license` subcommands
/// below should read from and write back to.
fn resolve_scope() -> Option<(Scope, LicensePaths)> {
    let machine_paths = store::machine_wide_paths();
    if store::load_activation_credentials(&machine_paths)
        .ok()
        .flatten()
        .is_some()
    {
        return Some((Scope::Machine, machine_paths));
    }
    let user_paths = store::default_paths();
    if store::load_activation_credentials(&user_paths)
        .ok()
        .flatten()
        .is_some()
    {
        return Some((Scope::User, user_paths));
    }
    None
}

/// Resolves the effective capability set for the current machine: a
/// verified, currently-valid licence's entitlement, or
/// [`Capabilities::community`] if there is none, it failed verification,
/// or it has expired. Never fails the caller -- an invalid licence must
/// never crash `cursdel`, only fall back to the free tier.
pub fn current_capabilities() -> Capabilities {
    match load_and_validate() {
        Some((_, entitlement, _)) => Capabilities::from_entitlement(&entitlement),
        None => Capabilities::community(),
    }
}

/// Checks machine-wide storage before per-user storage, since a
/// Deployment Key-enrolled machine's license is meant to apply to every
/// account on it. This is a narrow, local stand-in for the full
/// machine -> user -> Community precedence, which belongs to the separate
/// machine-wide storage/resolution work.
fn load_and_validate() -> Option<(VerifiedLicense, ProductEntitlement, Scope)> {
    for (scope, paths) in [
        (Scope::Machine, store::machine_wide_paths()),
        (Scope::User, store::default_paths()),
    ] {
        let Some(license_json) = store::load_license_file(&paths).ok().flatten() else {
            continue;
        };
        let Ok(verified) = cursdel_license::verify(&license_json) else {
            continue;
        };
        let Ok(entitlement) = cursdel_license::validate_product(
            &verified,
            cursdel_license::PRODUCT_CODE,
            None,
            None,
            None,
        ) else {
            continue;
        };
        return Some((verified, entitlement, scope));
    }
    None
}

fn status() -> ExitCode {
    println!("CurseDelete License Status\n");
    match load_and_validate() {
        Some((verified, entitlement, scope)) => {
            println!("License ID: {}", verified.data.license_id);
            println!("Customer:   {}", verified.data.customer);
            println!("Edition:    {}", entitlement.edition);
            println!("Type:       {}", entitlement.license_type);
            println!("Seats:      {}", entitlement.seats);
            println!("Scope:      {}", scope.label());
            if let Some(expires_at) = entitlement.expires_at {
                println!("Expires:    {expires_at}");
            }
            if let Some(activation) = &verified.data.activation {
                println!(
                    "Activation: {} ({} mode)",
                    activation.activation_id, activation.mode
                );
            }
        }
        None => {
            println!("No active license on this device -- running with Community capabilities.");
            println!();
            println!("Manual activation (License ID + Activation Code from your purchase email):");
            println!("  cursdel license activate --license-id <id> --activation-code <code>");
            println!();
            println!("Unattended enrollment (Deployment Key from your license administrator):");
            println!("  cursdel license enroll --deployment-key-env <ENV_VAR>");
        }
    }
    ExitCode::Success
}

fn activate(
    license_id: &str,
    activation_code: &str,
    offline: bool,
    output: Option<PathBuf>,
) -> ExitCode {
    let device = device::current();

    if offline {
        let request =
            cursdel_license::client::build_offline_activation_request(activation_code, &device);
        let path = output.unwrap_or_else(|| PathBuf::from("offline-activation-request.json"));
        let body = match serde_json::to_string_pretty(&request) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error: could not encode offline activation request: {e}");
                return ExitCode::UnexpectedFatal;
            }
        };
        match std::fs::write(&path, body) {
            Ok(()) => {
                println!("Offline activation request written to {}", path.display());
                println!();
                println!("Email this file to hello@repasscloud.com.");
                println!("Once you receive a signed license file back, run:");
                println!("  cursdel license import <path-to-license-file>");
                ExitCode::Success
            }
            Err(e) => {
                eprintln!(
                    "Error: could not write offline activation request to {}: {e}",
                    path.display()
                );
                ExitCode::UnexpectedFatal
            }
        }
    } else {
        let client = LicenseServerClient::new(server_base_url());
        match client.activate(license_id, activation_code, &device) {
            Ok((token, response)) => {
                let paths = store::default_paths();
                if let Err(e) = store::save_license_file(&paths, &response.signed_license) {
                    eprintln!("Error: could not save license file: {e}");
                    return ExitCode::UnexpectedFatal;
                }
                let creds = ActivationCredentials {
                    activation_id: response.activation_id.clone(),
                    activation_token: token,
                    mode: "online".to_string(),
                };
                if let Err(e) = store::save_activation_credentials(&paths, &creds) {
                    eprintln!("Error: could not save activation credentials: {e}");
                    return ExitCode::UnexpectedFatal;
                }
                println!("License activated successfully.");
                println!("License ID:   {}", response.license_id);
                println!("Activation:   {}", response.activation_id);
                ExitCode::Success
            }
            Err(e) => {
                eprintln!("Error: activation failed: {e}");
                ExitCode::LicenseRequired
            }
        }
    }
}

fn import(license_file: &Path) -> ExitCode {
    let text = match std::fs::read_to_string(license_file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: could not read {}: {e}", license_file.display());
            return ExitCode::CliUsageError;
        }
    };

    match cursdel_license::verify(&text) {
        Ok(_) => {
            let paths = store::default_paths();
            match store::save_license_file(&paths, &text) {
                Ok(()) => {
                    println!("License imported successfully.");
                    ExitCode::Success
                }
                Err(e) => {
                    eprintln!("Error: could not save license: {e}");
                    ExitCode::UnexpectedFatal
                }
            }
        }
        Err(e) => {
            eprintln!("Error: license file failed verification: {e}");
            ExitCode::LicenseRequired
        }
    }
}

fn deactivate() -> ExitCode {
    let Some((_scope, paths)) = resolve_scope() else {
        println!("No active activation found on this device.");
        return ExitCode::Success;
    };
    let Some(creds) = store::load_activation_credentials(&paths).ok().flatten() else {
        println!("No active activation found on this device.");
        return ExitCode::Success;
    };

    let device = device::current();
    let client = LicenseServerClient::new(server_base_url());
    match client.deactivate(
        &creds.activation_id,
        &creds.activation_token,
        &device.device_id,
    ) {
        Ok(_) => {
            // The server-side deactivation already succeeded at this
            // point, which is the state that actually matters (the seat
            // is freed for reactivation elsewhere); local cleanup is
            // best-effort but its failure is still worth telling the
            // operator about rather than silently leaving stale local
            // files that could otherwise look like this device is still
            // activated.
            if let Err(e) = store::remove_activation_credentials(&paths) {
                eprintln!("Warning: could not remove local activation credentials: {e}");
            }
            if let Err(e) = store::remove_license_file(&paths) {
                eprintln!("Warning: could not remove local license file: {e}");
            }
            println!("License deactivated on this device.");
            ExitCode::Success
        }
        Err(e) => {
            eprintln!("Error: deactivation failed: {e}");
            ExitCode::UnexpectedFatal
        }
    }
}

fn refresh() -> ExitCode {
    let Some((scope, paths)) = resolve_scope() else {
        eprintln!("Error: no active activation to refresh on this device.");
        return ExitCode::LicenseRequired;
    };
    let Some(creds) = store::load_activation_credentials(&paths).ok().flatten() else {
        eprintln!("Error: no active activation to refresh on this device.");
        return ExitCode::LicenseRequired;
    };

    if creds.mode != "online" {
        eprintln!(
            "Error: this device was activated offline, which has no lease to refresh. \
             Offline licenses do not need periodic refresh."
        );
        return ExitCode::LicenseRequired;
    }

    // Refresh uses only the stored activation credentials, never a
    // Deployment Key -- this is what lets revoking a Deployment Key stop
    // future enrollments without breaking machines already enrolled
    // through it.
    let device = device::current();
    let client = LicenseServerClient::new(server_base_url());
    match client.refresh(
        &creds.activation_id,
        &creds.activation_token,
        &device.device_id,
    ) {
        Ok(response) => {
            let save_result = match scope {
                Scope::Machine => {
                    store::save_machine_license_file(&paths, &response.signed_license)
                }
                Scope::User => store::save_license_file(&paths, &response.signed_license),
            };
            match save_result {
                Ok(()) => {
                    println!("License refreshed.");
                    ExitCode::Success
                }
                Err(e) => {
                    eprintln!("Error: could not save refreshed license: {e}");
                    ExitCode::UnexpectedFatal
                }
            }
        }
        Err(e) => {
            eprintln!("Error: refresh failed: {e}");
            ExitCode::UnexpectedFatal
        }
    }
}

/// Collects the Deployment Key from exactly one of the supported input
/// sources. `clap`'s `conflicts_with_all` on each flag already guarantees
/// at most one was supplied; this only needs to handle "none supplied".
fn resolve_deployment_key(
    deployment_key: Option<String>,
    deployment_key_env: Option<String>,
    deployment_key_file: Option<PathBuf>,
    deployment_key_stdin: bool,
) -> Result<Secret, String> {
    if let Some(value) = deployment_key {
        return Ok(Secret::new(value));
    }
    if let Some(var) = deployment_key_env {
        return std::env::var(&var)
            .map(Secret::new)
            .map_err(|_| format!("environment variable '{var}' is not set."));
    }
    if let Some(path) = deployment_key_file {
        return std::fs::read_to_string(&path)
            .map(|s| Secret::new(s.trim().to_string()))
            .map_err(|e| format!("could not read '{}': {e}", path.display()));
    }
    if deployment_key_stdin {
        let mut buf = String::new();
        return std::io::stdin()
            .read_to_string(&mut buf)
            .map(|_| Secret::new(buf.trim().to_string()))
            .map_err(|e| format!("could not read Deployment Key from stdin: {e}"));
    }
    Err(
        "one of --deployment-key, --deployment-key-env, --deployment-key-file, or \
         --deployment-key-stdin is required."
            .to_string(),
    )
}

fn enroll(
    deployment_key: Option<String>,
    deployment_key_env: Option<String>,
    deployment_key_file: Option<PathBuf>,
    deployment_key_stdin: bool,
) -> ExitCode {
    let deployment_key = match resolve_deployment_key(
        deployment_key,
        deployment_key_env,
        deployment_key_file,
        deployment_key_stdin,
    ) {
        Ok(key) => key,
        Err(msg) => {
            eprintln!("Error: {msg}");
            return ExitCode::CliUsageError;
        }
    };

    if deployment_key.expose().trim().is_empty() {
        eprintln!("Error: the supplied Deployment Key is empty.");
        return ExitCode::CliUsageError;
    }

    let device = device::current();
    let paths = store::machine_wide_paths();

    // Idempotency: a machine already enrolled with valid local state
    // reports success without contacting the server, so re-running
    // `license enroll` (e.g. from an idempotent provisioning script)
    // never consumes another seat.
    if let Some(license_json) = store::load_license_file(&paths).ok().flatten() {
        if let Ok(verified) = cursdel_license::verify(&license_json) {
            if cursdel_license::validate_activation(&verified, Some(&device), None).is_ok()
                && cursdel_license::validate_product(
                    &verified,
                    cursdel_license::PRODUCT_CODE,
                    None,
                    None,
                    None,
                )
                .is_ok()
            {
                println!("This machine is already enrolled and its licence is valid.");
                println!("License ID: {}", verified.data.license_id);
                return ExitCode::Success;
            }
        }
    }

    // Fail before touching the network if this process cannot establish
    // machine-wide state -- a failed enrollment must never silently fall
    // back to user-scoped activation.
    if let Err(e) = store::ensure_machine_wide_writable(&paths) {
        eprintln!(
            "Error: cannot write machine-wide licence state at {}: {e}\n\
             Re-run with administrator/root privileges.",
            paths.license_file.display()
        );
        return ExitCode::PrivilegeRequirementNotSatisfied;
    }

    let client = LicenseServerClient::new(server_base_url());
    let (token, response) = match client.enroll(&deployment_key, &device) {
        Ok(pair) => pair,
        Err(e) => return report_enroll_error(&e),
    };
    // The Deployment Key has now served its purpose (this one enrollment
    // request); it goes out of scope at the end of this function and its
    // backing buffer is best-effort zeroed by `Secret`'s `Drop` impl. It
    // is never written to `token`, `response`, or any file below.

    let verified = match cursdel_license::verify(&response.signed_license) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: server returned a licence that failed local verification: {e}");
            return ExitCode::UnexpectedFatal;
        }
    };

    if let Err(e) = cursdel_license::validate_activation(&verified, Some(&device), None) {
        eprintln!("Error: the licence returned by the server is not bound to this machine: {e}");
        return ExitCode::LicenseRequired;
    }
    if let Err(e) = cursdel_license::validate_product(
        &verified,
        cursdel_license::PRODUCT_CODE,
        None,
        None,
        None,
    ) {
        eprintln!("Error: the licence returned by the server does not cover this product: {e}");
        return ExitCode::LicenseRequired;
    }

    if let Err(e) = store::save_machine_license_file(&paths, &response.signed_license) {
        eprintln!(
            "Error: could not write machine-wide licence file at {}: {e}",
            paths.license_file.display()
        );
        return ExitCode::PrivilegeRequirementNotSatisfied;
    }
    let creds = ActivationCredentials {
        activation_id: response.activation_id.clone(),
        activation_token: token,
        mode: "online".to_string(),
    };
    if let Err(e) = store::save_machine_activation_credentials(&paths, &creds) {
        eprintln!(
            "Error: could not write machine-wide activation credentials at {}: {e}",
            paths.activation_credentials_file.display()
        );
        return ExitCode::PrivilegeRequirementNotSatisfied;
    }

    println!("Machine enrolled successfully via Deployment Key.");
    println!("License ID:   {}", response.license_id);
    println!("Activation:   {}", response.activation_id);
    println!("Scope:        machine-wide");
    ExitCode::Success
}

/// Maps a Deployment Key enrollment failure onto a distinct, actionable
/// message and exit code. Never includes the Deployment Key itself --
/// `ClientError` never carries it in the first place (see
/// `LicenseServerClient::enroll`), only whatever the server chose to put
/// in its Problem Details response.
fn report_enroll_error(e: &ClientError) -> ExitCode {
    match e {
        ClientError::Rejected {
            status,
            title,
            detail,
        } => match *status {
            400 => {
                eprintln!("Error: enrollment request was rejected: {title}{detail}");
                ExitCode::CliUsageError
            }
            401 => {
                eprintln!(
                    "Error: Deployment Key rejected: {title}{detail}\n\
                     Contact your licence administrator for a valid Deployment Key."
                );
                ExitCode::LicenseRequired
            }
            409 => {
                eprintln!(
                    "Error: {title}{detail}\n\
                     Contact your licence administrator or RePass Cloud support."
                );
                ExitCode::LicenseRequired
            }
            429 => {
                eprintln!("Error: rate limited by the licence server: {title}{detail}\nWait a moment and try again.");
                ExitCode::UnexpectedFatal
            }
            503 => {
                eprintln!(
                    "Error: licence server temporarily unavailable: {title}{detail}\nTry again later."
                );
                ExitCode::UnexpectedFatal
            }
            _ => {
                eprintln!("Error: enrollment failed: {title}{detail} (status {status})");
                ExitCode::UnexpectedFatal
            }
        },
        ClientError::Network(_) => {
            eprintln!("Error: could not reach the licence server: {e}");
            ExitCode::UnexpectedFatal
        }
        ClientError::Decode(_) => {
            eprintln!("Error: unexpected response from the licence server: {e}");
            ExitCode::UnexpectedFatal
        }
    }
}
