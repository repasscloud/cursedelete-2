//! Public signing keys trusted by this build, mirroring
//! `src/Licensing.Core/TrustedPublicKeys.cs` in
//! `danijeljw-RPC/licsense-server-poc` byte-for-byte. These are public
//! keys -- embedding them is the entire point of a self-contained
//! offline verifier, and does not require or reveal any private signing
//! material.
//!
//! Add future public keys here before the server issues licences signed
//! with their key IDs, and keep old public keys so perpetual licences
//! continue to validate. Pin/bump deliberately when told a key rotation
//! happened; do not silently drop an old entry.

use std::collections::HashMap;

pub fn trusted_public_keys() -> HashMap<&'static str, &'static str> {
    let mut keys = HashMap::new();
    keys.insert(
        "primary-2026",
        "-----BEGIN PUBLIC KEY-----\n\
         MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEXXu3cKxsg4XRn3w+DBklf2uL1Zzm\n\
         2ZS9bU3kyD7SY5AYM5fVdBAavnS4esT4dpBU1sV4RLPfVuH2Vu7f7BDXtQ==\n\
         -----END PUBLIC KEY-----\n",
    );
    keys.insert(
        "secondary-2026",
        "-----BEGIN PUBLIC KEY-----\n\
         MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEqlkXV3jGhEOCaRq6aD11pYmXVNgv\n\
         6rJS/aY9WtJHHH6kaTKFxgj1eu8a7Qw4fVh+wRnFd4T29taRbKDxgmPpaA==\n\
         -----END PUBLIC KEY-----\n",
    );
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_expected_key_ids() {
        let keys = trusted_public_keys();
        assert!(keys.contains_key("primary-2026"));
        assert!(keys.contains_key("secondary-2026"));
    }

    #[test]
    fn keys_parse_as_valid_p256_public_keys() {
        for (id, pem) in trusted_public_keys() {
            let result =
                <p256::PublicKey as p256::pkcs8::DecodePublicKey>::from_public_key_pem(pem);
            assert!(result.is_ok(), "key {id} failed to parse: {result:?}");
        }
    }
}
