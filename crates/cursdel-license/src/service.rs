//! The boundary between an application and the (separately developed)
//! licensing server.
//!
//! Applications talk to the server only through [`LicenseService`], so
//! local validation never depends on it and tests can substitute a fake.
//! The server's responses are never trusted on their own: everything it
//! returns is a signed envelope that the caller validates with
//! [`crate::LicensePolicy`] before storing or using it.
//!
//! The wire contract implemented by [`crate::http::HttpLicenseService`] is
//! **provisional** -- see `docs/LICENSING-SERVER-CONTRACT.md`.

/// A signed licence and, for Business, its signed activation certificate,
/// both still unverified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub license: String,
    pub activation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Product<'a> {
    pub product_id: &'a str,
    pub major_version: u32,
}

/// Proof that this installation owns a Business activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationRef<'a> {
    pub activation_id: &'a str,
    pub device_id: &'a str,
    pub client_secret: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivateRequest<'a> {
    pub product: Product<'a>,
    /// The activation key or token the customer was given.
    pub activation_key: &'a str,
    /// Sent so a Business activation can be bound to this device. The
    /// server ignores it for editions without device activation.
    pub device_id: &'a str,
    /// Random secret this installation will use to renew/deactivate.
    pub client_secret: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenewRequest<'a> {
    pub product: Product<'a>,
    pub license_id: &'a str,
    /// Present for an activated Business licence.
    pub activation: Option<ActivationRef<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// No server endpoint is configured for this build/environment.
    #[error("no licensing server is configured")]
    NotConfigured,
    /// Network failure, timeout, or server-side error. "Couldn't check
    /// right now" -- never a reason to invalidate an existing licence.
    #[error("licensing server unavailable: {0}")]
    Unavailable(String),
    #[error("the licence has been revoked: {0}")]
    Revoked(String),
    /// A definitive refusal (unknown key, seat or transfer limit reached,
    /// wrong product, ...). `code` is the server's machine-readable code.
    #[error("licensing server refused the request: {message} ({code})")]
    Rejected { code: String, message: String },
    #[error("unexpected response from licensing server: {0}")]
    Protocol(String),
}

/// Machine-readable [`ServiceError::Rejected`] code meaning the activation
/// is already inactive server-side (e.g. released through the portal).
pub const CODE_ACTIVATION_INACTIVE: &str = "activation_inactive";

pub trait LicenseService {
    /// Obtain a licence from an activation key. For Business this also
    /// consumes (or re-uses, for the same device) an activation seat and
    /// returns a certificate; for other editions it simply provisions the
    /// licence.
    fn activate(&self, request: &ActivateRequest<'_>) -> Result<Grant, ServiceError>;

    /// Fetch the current signed entitlement for an existing licence (new
    /// subscription expiry, re-issued certificate, ...).
    fn renew(&self, request: &RenewRequest<'_>) -> Result<Grant, ServiceError>;

    /// Release a Business activation seat.
    fn deactivate(
        &self,
        product: Product<'_>,
        activation: &ActivationRef<'_>,
    ) -> Result<(), ServiceError>;
}

/// Used when no endpoint is configured: every call fails with
/// [`ServiceError::NotConfigured`], and offline licensing keeps working.
pub struct UnconfiguredService;

impl LicenseService for UnconfiguredService {
    fn activate(&self, _: &ActivateRequest<'_>) -> Result<Grant, ServiceError> {
        Err(ServiceError::NotConfigured)
    }

    fn renew(&self, _: &RenewRequest<'_>) -> Result<Grant, ServiceError> {
        Err(ServiceError::NotConfigured)
    }

    fn deactivate(&self, _: Product<'_>, _: &ActivationRef<'_>) -> Result<(), ServiceError> {
        Err(ServiceError::NotConfigured)
    }
}
