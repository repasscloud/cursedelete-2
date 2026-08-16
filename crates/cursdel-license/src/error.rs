//! Mirrors `Licensing.Core`'s two exception types: a malformed licence
//! (`LicenseSchemaException`) versus a well-formed licence that is not
//! currently valid to use (`LicenseValidationException` -- bad signature,
//! wrong device, expired, no entitlement, expired lease, ...). Keeping
//! them distinct matches the upstream contract and lets callers treat
//! "tampered/corrupted file" differently from "valid but not currently
//! usable" if they want to.

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SchemaError(pub String);

impl SchemaError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Schema(#[from] SchemaError),
}

impl ValidationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
