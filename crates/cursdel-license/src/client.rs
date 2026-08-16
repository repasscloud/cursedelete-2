//! Online activation HTTP client, matching the exact request/response
//! shapes documented in `LICENSING-INTEGRATION.md` §6 (copied verbatim
//! from the license server's own `ApiContracts.cs`, which are internal to
//! that project and not shipped as a package -- this crate defines
//! matching DTOs as instructed).
//!
//! Network failures are deliberately distinguished from licence
//! invalidity: per §9, "don't treat a network failure as 'license
//! invalid', treat it as 'couldn't check right now'". Callers should fall
//! back to the last-verified local licence file on [`ClientError::Network`]
//! rather than treating it the same as a rejected activation.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::device::LocalDeviceIdentity;

pub const DEFAULT_DEV_SERVER_URL: &str = "http://localhost:8080";

/// Environment variable used to configure the license server base URL.
/// A production URL is a deployment-time configuration value, not
/// something to hardcode here -- see `docs/adr/0004-licensing-integration.md`.
pub const SERVER_URL_ENV_VAR: &str = "CURSDEL_LICENSE_SERVER_URL";

pub fn server_base_url() -> String {
    std::env::var(SERVER_URL_ENV_VAR).unwrap_or_else(|_| DEFAULT_DEV_SERVER_URL.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("network error contacting license server: {0}")]
    Network(#[from] Box<ureq::Error>),
    #[error("unexpected response from license server: {0}")]
    Decode(String),
    #[error("license server rejected the request: {title} ({status}){detail}")]
    Rejected {
        status: u16,
        title: String,
        detail: String,
    },
}

impl From<ureq::Error> for ClientError {
    fn from(e: ureq::Error) -> Self {
        ClientError::Network(Box::new(e))
    }
}

#[derive(Serialize)]
struct DeviceRequest<'a> {
    scheme: &'a str,
    #[serde(rename = "deviceId")]
    device_id: &'a str,
    #[serde(rename = "deviceName", skip_serializing_if = "Option::is_none")]
    device_name: Option<&'a str>,
}

#[derive(Serialize)]
struct ActivateRequest<'a> {
    #[serde(rename = "requestId")]
    request_id: String,
    #[serde(rename = "activationCode")]
    activation_code: &'a str,
    #[serde(rename = "activationToken")]
    activation_token: &'a str,
    mode: &'a str,
    device: DeviceRequest<'a>,
}

#[derive(Serialize)]
struct CredentialRequest<'a> {
    #[serde(rename = "activationToken")]
    activation_token: &'a str,
    #[serde(rename = "deviceId")]
    device_id: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivationResponse {
    #[serde(rename = "licenseId")]
    pub license_id: String,
    #[serde(rename = "activationId")]
    pub activation_id: String,
    pub status: String,
    #[serde(rename = "signedLicense")]
    pub signed_license: String,
    #[serde(rename = "refreshAfter")]
    pub refresh_after: Option<String>,
    #[serde(rename = "leaseExpiresAt")]
    pub lease_expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationResponse {
    #[serde(rename = "licenseId")]
    pub license_id: String,
    #[serde(rename = "activationId")]
    pub activation_id: String,
    pub status: String,
    #[serde(rename = "serverTime")]
    pub server_time: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeactivationResponse {
    #[serde(rename = "licenseId")]
    pub license_id: String,
    #[serde(rename = "activationId")]
    pub activation_id: String,
    pub status: String,
    #[serde(rename = "deactivatedAt")]
    pub deactivated_at: String,
}

pub struct LicenseServerClient {
    base_url: String,
}

impl LicenseServerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    /// Activates a licence. `mode` is `"online"`. Returns the freshly
    /// generated `activation_token` (persist it -- the server never
    /// returns it, it only ever echoes back what the caller already has)
    /// alongside the server's response.
    pub fn activate(
        &self,
        license_id: &str,
        activation_code: &str,
        device: &LocalDeviceIdentity,
    ) -> Result<(String, ActivationResponse), ClientError> {
        let activation_token = generate_activation_token();
        let request = ActivateRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            activation_code,
            activation_token: &activation_token,
            mode: "online",
            device: DeviceRequest {
                scheme: device.scheme,
                device_id: &device.device_id,
                device_name: Some(&device.device_name),
            },
        };

        let url = format!(
            "{}/api/v1/licenses/{}/activate",
            self.base_url.trim_end_matches('/'),
            license_id
        );
        let response = self.post_json(&url, &request)?;
        Ok((activation_token, response))
    }

    pub fn validate(
        &self,
        activation_id: &str,
        activation_token: &str,
        device_id: &str,
    ) -> Result<ValidationResponse, ClientError> {
        let request = CredentialRequest {
            activation_token,
            device_id,
        };
        let url = format!(
            "{}/api/v1/activations/{}/validate",
            self.base_url.trim_end_matches('/'),
            activation_id
        );
        self.post_json(&url, &request)
    }

    pub fn refresh(
        &self,
        activation_id: &str,
        activation_token: &str,
        device_id: &str,
    ) -> Result<ActivationResponse, ClientError> {
        let request = CredentialRequest {
            activation_token,
            device_id,
        };
        let url = format!(
            "{}/api/v1/activations/{}/refresh",
            self.base_url.trim_end_matches('/'),
            activation_id
        );
        self.post_json(&url, &request)
    }

    pub fn deactivate(
        &self,
        activation_id: &str,
        activation_token: &str,
        device_id: &str,
    ) -> Result<DeactivationResponse, ClientError> {
        let request = CredentialRequest {
            activation_token,
            device_id,
        };
        let url = format!(
            "{}/api/v1/activations/{}/deactivate",
            self.base_url.trim_end_matches('/'),
            activation_id
        );
        self.post_json(&url, &request)
    }

    fn post_json<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<R, ClientError> {
        match ureq::post(url).send_json(body) {
            Ok(response) => response
                .into_json::<R>()
                .map_err(|e| ClientError::Decode(e.to_string())),
            Err(ureq::Error::Status(status, response)) => {
                let problem: ProblemDetails = response.into_json().unwrap_or(ProblemDetails {
                    title: "request rejected".to_string(),
                    detail: None,
                });
                Err(ClientError::Rejected {
                    status,
                    title: problem.title,
                    detail: problem.detail.map(|d| format!(": {d}")).unwrap_or_default(),
                })
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProblemDetails {
    #[serde(default = "default_title")]
    title: String,
    detail: Option<String>,
}

fn default_title() -> String {
    "request rejected".to_string()
}

fn generate_activation_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Builds the offline activation request JSON per §7 -- same shape as the
/// online request, `mode: "offline"`, written to a file instead of POSTed.
pub fn build_offline_activation_request(
    activation_code: &str,
    device: &LocalDeviceIdentity,
) -> serde_json::Value {
    let activation_token = generate_activation_token();
    serde_json::json!({
        "requestId": uuid::Uuid::new_v4().to_string(),
        "activationCode": activation_code,
        "activationToken": activation_token,
        "mode": "offline",
        "device": {
            "scheme": device.scheme,
            "deviceId": device.device_id,
            "deviceName": device.device_name,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_token_is_32_bytes_base64_encoded() {
        let token = generate_activation_token();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&token)
            .unwrap();
        assert_eq!(decoded.len(), 32);
    }

    // NOTE: mutates the real process environment (`std::env::remove_var`),
    // which is shared, unsynchronised global state across every test
    // thread in this binary (`cargo test` runs tests as threads within
    // one process, not separate processes). Safe today because no other
    // test in this crate reads `SERVER_URL_ENV_VAR` -- if a future test
    // needs to depend on that variable's value, give it its own
    // dependency-injected override instead of adding another env mutation
    // here, to avoid a real cross-test race.
    #[test]
    fn default_server_url_is_documented_dev_url_when_env_unset() {
        std::env::remove_var(SERVER_URL_ENV_VAR);
        assert_eq!(server_base_url(), DEFAULT_DEV_SERVER_URL);
    }

    #[test]
    fn offline_request_has_offline_mode_and_valid_token() {
        let device = LocalDeviceIdentity {
            scheme: "os-machine-id-sha256-v1",
            device_id: "A".repeat(64),
            device_name: "test".to_string(),
            source: "test",
        };
        let request = build_offline_activation_request("CODE-1", &device);
        assert_eq!(request["mode"], "offline");
        assert_eq!(request["activationCode"], "CODE-1");
        assert!(!request["activationToken"].as_str().unwrap().is_empty());
    }
}
