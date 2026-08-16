# Test fixtures

These are **not** production licences and were **not** signed with
RePassCloud's real signing key. They were generated locally with the real
`LicenseGenerator` CLI from `danijeljw-RPC/licsense-server-poc`, using a
disposable ECDSA P-256 keypair (`test2026`) created solely for this
purpose and discarded afterward -- the private key never left the
generating machine and is not checked in.

Their purpose is byte-exact interop validation: `cursdel-license`'s
canonical-JSON serialisation and ECDSA-P256-SHA256/IEEE-P1363 signature
verification must produce the same answer the real C# `Licensing.Core`
implementation would, including for the exhaustive printable-ASCII +
control-character + non-ASCII payload in `exhaustive.license`, which
pins down .NET's `Utf8JsonWriter` default string-escaping table
empirically (it is considerably more aggressive than bare JSON requires
-- e.g. `"` is written as `"` rather than `\"`, and `&`, `'`, `+`,
`<`, `>`, and backtick are all escaped even though JSON does not require
it). See `crates/cursdel-license/src/canonical_json.rs` for the resulting
implementation and `docs/adr/0004-licensing-integration.md` for why this
mattered.

- `test2026.public.pem` -- the disposable test public key.
- `test.license` -- a small signed licence with Unicode/HTML-sensitive
  characters in `customer` and `metadata`.
- `exhaustive.license` -- a signed licence whose `customer` field contains
  every printable ASCII character, every C0 control character with a
  standard JSON shorthand escape, and a sample of non-ASCII/astral
  characters (accented Latin, an emoji with a variation selector, and the
  U+2028/U+2029 line/paragraph separators).
