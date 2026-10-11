//! HTTPS implementation of [`LicenseService`] against the **provisional**
//! contract in `docs/LICENSING-SERVER-CONTRACT.md`. When the replacement
//! server's real API is settled, only this module should need to change.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::service::{
    ActivateRequest, ActivationRef, Grant, LicenseService, Product, RenewRequest, ServiceError,
};

const TIMEOUT: Duration = Duration::from_secs(30);

pub struct HttpLicenseService {
    base_url: String,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("licensing server URL must use https:// (plain http is only allowed for localhost): {0}")]
pub struct InsecureUrl(pub String);

impl HttpLicenseService {
    /// Rejects anything but `https://`, except `http://` to a loopback
    /// host for local development.
    pub fn new(base_url: &str) -> Result<Self, InsecureUrl> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        if !is_acceptable_url(&base_url) {
            return Err(InsecureUrl(base_url));
        }
        Ok(Self {
            base_url,
            agent: ureq::AgentBuilder::new().timeout(TIMEOUT).build(),
        })
    }

    fn post<B: Serialize>(&self, path: &str, body: &B) -> Result<Value, ServiceError> {
        let url = format!("{}{path}", self.base_url);
        match self.agent.post(&url).send_json(body) {
            Ok(response) if response.status() == 204 => Ok(Value::Null),
            Ok(response) => response
                .into_json::<Value>()
                .map_err(|e| ServiceError::Protocol(format!("invalid JSON response: {e}"))),
            Err(ureq::Error::Status(status, response)) => {
                let problem: ErrorBody = response.into_json().unwrap_or_default();
                let message = problem.message.unwrap_or_else(|| format!("HTTP {status}"));
                let code = problem.error.unwrap_or_else(|| format!("http_{status}"));
                Err(if status >= 500 || status == 429 {
                    ServiceError::Unavailable(message)
                } else if code == "revoked" {
                    ServiceError::Revoked(message)
                } else {
                    ServiceError::Rejected { code, message }
                })
            }
            Err(e) => Err(ServiceError::Unavailable(e.to_string())),
        }
    }
}

fn is_acceptable_url(url: &str) -> bool {
    if url.starts_with("https://") && url.len() > "https://".len() {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| {
        rest.strip_prefix(host)
            .is_some_and(|after| after.is_empty() || after.starts_with([':', '/']))
    })
}

/// Identifiers are interpolated into URL paths; they come from signed
/// claims, but are still restricted to a URL-safe alphabet.
fn path_segment(id: &str) -> Result<&str, ServiceError> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        Ok(id)
    } else {
        Err(ServiceError::Protocol(format!(
            "identifier '{id}' is not URL-safe"
        )))
    }
}

#[derive(Default, Deserialize)]
struct ErrorBody {
    error: Option<String>,
    message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivateBody<'a> {
    product_id: &'a str,
    major_version: u32,
    activation_key: &'a str,
    device_id: &'a str,
    client_secret: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivationBody<'a> {
    activation_id: &'a str,
    device_id: &'a str,
    client_secret: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenewBody<'a> {
    product_id: &'a str,
    major_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation: Option<ActivationBody<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeactivateBody<'a> {
    product_id: &'a str,
    major_version: u32,
    device_id: &'a str,
    client_secret: &'a str,
}

impl<'a> From<&ActivationRef<'a>> for ActivationBody<'a> {
    fn from(a: &ActivationRef<'a>) -> Self {
        Self {
            activation_id: a.activation_id,
            device_id: a.device_id,
            client_secret: a.client_secret,
        }
    }
}

/// `{"license": <envelope>, "activation": <envelope> | null}`
fn parse_grant(value: Value) -> Result<Grant, ServiceError> {
    let envelope = |v: &Value| -> Result<String, ServiceError> {
        if v.is_object() {
            serde_json::to_string_pretty(v).map_err(|e| ServiceError::Protocol(e.to_string()))
        } else {
            Err(ServiceError::Protocol(
                "expected a signed envelope object".into(),
            ))
        }
    };
    let license = value
        .get("license")
        .ok_or_else(|| ServiceError::Protocol("response has no 'license'".into()))
        .and_then(envelope)?;
    let activation = match value.get("activation") {
        None | Some(Value::Null) => None,
        Some(v) => Some(envelope(v)?),
    };
    Ok(Grant {
        license,
        activation,
    })
}

impl LicenseService for HttpLicenseService {
    fn activate(&self, r: &ActivateRequest<'_>) -> Result<Grant, ServiceError> {
        let body = ActivateBody {
            product_id: r.product.product_id,
            major_version: r.product.major_version,
            activation_key: r.activation_key,
            device_id: r.device_id,
            client_secret: r.client_secret,
        };
        parse_grant(self.post("/v1/activations", &body)?)
    }

    fn renew(&self, r: &RenewRequest<'_>) -> Result<Grant, ServiceError> {
        let body = RenewBody {
            product_id: r.product.product_id,
            major_version: r.product.major_version,
            activation: r.activation.as_ref().map(ActivationBody::from),
        };
        let path = format!("/v1/licenses/{}/renew", path_segment(r.license_id)?);
        parse_grant(self.post(&path, &body)?)
    }

    fn deactivate(
        &self,
        product: Product<'_>,
        activation: &ActivationRef<'_>,
    ) -> Result<(), ServiceError> {
        let body = DeactivateBody {
            product_id: product.product_id,
            major_version: product.major_version,
            device_id: activation.device_id,
            client_secret: activation.client_secret,
        };
        let path = format!(
            "/v1/activations/{}/deactivate",
            path_segment(activation.activation_id)?
        );
        self.post(&path, &body).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_https_or_loopback_http_is_accepted() {
        for ok in [
            "https://licensing.example.com",
            "https://licensing.example.com/",
            "http://localhost:8080",
            "http://127.0.0.1",
            "http://[::1]:9000/api",
        ] {
            assert!(HttpLicenseService::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://licensing.example.com",
            "http://localhost.evil.com",
            "ftp://x",
            "https://",
            "",
        ] {
            assert!(HttpLicenseService::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn grant_parsing_requires_envelope_objects() {
        let g = parse_grant(json!({"license": {"format": "x"}, "activation": null})).unwrap();
        assert!(g.license.contains("\"format\""));
        assert_eq!(g.activation, None);
        assert!(parse_grant(json!({"activation": {}})).is_err());
        assert!(parse_grant(json!({"license": "string"})).is_err());
    }

    #[test]
    fn path_segments_must_be_url_safe() {
        assert!(path_segment("lic_01.A-b").is_ok());
        assert!(path_segment("../admin").is_err());
        assert!(path_segment("").is_err());
    }

    #[test]
    fn unreachable_server_is_unavailable_not_rejected() {
        // Port 9 (discard) on loopback: nothing listens there.
        let svc = HttpLicenseService::new("http://127.0.0.1:9").unwrap();
        let err = svc
            .deactivate(
                Product {
                    product_id: "app",
                    major_version: 2,
                },
                &ActivationRef {
                    activation_id: "act_1",
                    device_id: "D",
                    client_secret: "s",
                },
            )
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unavailable(_)), "{err:?}");
    }
}
