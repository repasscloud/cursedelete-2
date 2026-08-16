//! `cursdel license status|activate|import|deactivate|refresh`.
//!
//! Every command here fails closed to a clear message rather than
//! crashing: a licence problem is always reported as "not activated"
//! with an explanation, never a panic (`LICENSING-INTEGRATION.md` §9).
//! Activation bearer tokens are never printed.

use std::path::{Path, PathBuf};

use cursdel_core::exit_code::ExitCode;
use cursdel_license::client::{server_base_url, LicenseServerClient};
use cursdel_license::store::{self, ActivationCredentials};
use cursdel_license::{device, ProductEntitlement, VerifiedLicense};
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
    }
}

/// Resolves the effective capability set for the current machine: a
/// verified, currently-valid licence's entitlement, or
/// [`Capabilities::community`] if there is none, it failed verification,
/// or it has expired. Never fails the caller -- an invalid licence must
/// never crash `cursdel`, only fall back to the free tier.
pub fn current_capabilities() -> Capabilities {
    match load_and_validate() {
        Some((_, entitlement)) => Capabilities::from_entitlement(&entitlement),
        None => Capabilities::community(),
    }
}

fn load_and_validate() -> Option<(VerifiedLicense, ProductEntitlement)> {
    let paths = store::default_paths();
    let license_json = store::load_license_file(&paths).ok()??;
    let verified = cursdel_license::verify(&license_json).ok()?;
    let entitlement = cursdel_license::validate_product(
        &verified,
        cursdel_license::PRODUCT_CODE,
        None,
        None,
        None,
    )
    .ok()?;
    Some((verified, entitlement))
}

fn status() -> ExitCode {
    println!("CurseDelete License Status\n");
    match load_and_validate() {
        Some((verified, entitlement)) => {
            println!("License ID: {}", verified.data.license_id);
            println!("Customer:   {}", verified.data.customer);
            println!("Edition:    {}", entitlement.edition);
            println!("Type:       {}", entitlement.license_type);
            println!("Seats:      {}", entitlement.seats);
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
            println!("Activate with:");
            println!("  cursdel license activate --license-id <id> --activation-code <code>");
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
    let paths = store::default_paths();
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
            let _ = store::remove_activation_credentials(&paths);
            let _ = store::remove_license_file(&paths);
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
    let paths = store::default_paths();
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

    let device = device::current();
    let client = LicenseServerClient::new(server_base_url());
    match client.refresh(
        &creds.activation_id,
        &creds.activation_token,
        &device.device_id,
    ) {
        Ok(response) => match store::save_license_file(&paths, &response.signed_license) {
            Ok(()) => {
                println!("License refreshed.");
                ExitCode::Success
            }
            Err(e) => {
                eprintln!("Error: could not save refreshed license: {e}");
                ExitCode::UnexpectedFatal
            }
        },
        Err(e) => {
            eprintln!("Error: refresh failed: {e}");
            ExitCode::UnexpectedFatal
        }
    }
}
