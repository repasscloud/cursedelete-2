//! Rust-native, offline-capable verifier and activation client for the
//! `software-license-v1` envelope defined by `danijeljw-RPC/licsense-server-poc`'s
//! `Licensing.Core` package. See `LICENSING-INTEGRATION.md` for the full
//! protocol and `docs/adr/0004-licensing-integration.md` for why this
//! crate exists instead of a NativeAOT/sidecar bridge to the real .NET
//! package.

pub mod canonical_json;
pub mod client;
pub mod device;
pub mod error;
pub mod schema;
pub mod secret;
pub mod store;
pub mod trusted_keys;
pub mod verify;

pub use device::LocalDeviceIdentity;
pub use error::{SchemaError, ValidationError};
pub use schema::{ActivationData, DeviceBinding, LicenseData, ProductEntitlement};
pub use secret::Secret;
pub use verify::{validate_activation, validate_product, verify, verify_file, VerifiedLicense};

/// The stable product code CurseDelete registers on the licence server and
/// matches against every entitlement's `product` field. Fixed per the
/// product brief -- do not change without a corresponding server-side
/// product registration.
pub const PRODUCT_CODE: &str = "cursedelete";
