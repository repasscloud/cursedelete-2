//! A minimal wrapper for in-memory secrets (currently only the Deployment
//! Key -- see `client::LicenseServerClient::enroll`). It exists to make
//! accidental exposure structurally harder rather than relying on call
//! sites to remember not to print/log it:
//!
//! - `Debug`/`Display` never render the wrapped value, so it is safe
//!   inside a `#[derive(Debug)]` struct, a `panic!("{:?}", ...)`, or a
//!   `tracing` field without leaking the secret.
//! - `Drop` best-effort zeroes the backing buffer so the plaintext value
//!   doesn't linger in freed memory longer than necessary.
//!
//! This is intentionally small rather than pulling in the `zeroize` /
//! `secrecy` ecosystem: the Deployment Key lives for a handful of
//! statements (parse it, put it in one JSON request body, drop it) and
//! never touches disk, so a dependency-free wrapper covers the actual
//! requirement ("never persist it, never log it, clear it when reasonably
//! practical") without adding supply-chain surface for it.

pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(REDACTED)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("REDACTED")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: writing arbitrary bytes (including 0x00, itself valid
        // UTF-8) into a `String`'s existing, already-allocated backing
        // buffer never changes its length or capacity, so the `String`
        // remains a valid (if meaningless) UTF-8 string for the remainder
        // of this call -- it is dropped immediately afterwards and never
        // read again. `write_volatile` (rather than a plain assignment)
        // discourages the optimizer from eliding the writes as dead
        // stores just because the buffer is about to be freed.
        unsafe {
            let bytes = self.0.as_bytes_mut();
            for byte in bytes.iter_mut() {
                std::ptr::write_volatile(byte, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_reveal_the_value() {
        let secret = Secret::new("dpk_live_super_secret".to_string());
        assert!(!format!("{secret:?}").contains("super_secret"));
        assert!(!format!("{secret}").contains("super_secret"));
    }

    #[test]
    fn expose_returns_the_original_value() {
        let secret = Secret::new("dpk_live_abc".to_string());
        assert_eq!(secret.expose(), "dpk_live_abc");
    }
}
