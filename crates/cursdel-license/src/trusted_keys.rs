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
         MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEfI5MgZP+c6rTxr2wABqPIHlqE5Cf\n\
         wy5HMrUTcnJfdj/ksm1TLmvbPJF6GJ+N6PlTCdGe0vssSBTuPbFOZEDrSQ==\n\
         -----END PUBLIC KEY-----\n",
    );
    keys.insert(
        "secondary-2026",
        "-----BEGIN PUBLIC KEY-----\n\
         MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEJ0LLdDYwckz5M6XJf3oWgcvyAKec\n\
         B7gLmxTtszqG6sN9aQkV1oI0Yo/KhZpyP/u0E7iGKSkxiT+sH6nJo5w7Ew==\n\
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
