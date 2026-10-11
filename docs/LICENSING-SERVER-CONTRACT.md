# Licensing server contract (provisional)

> **Status: provisional.** This is what CurseDelete's application-side
> licensing currently sends and expects. The replacement licensing server
> has not been built yet; when it is, this contract may change. Only
> [`crates/cursdel-license/src/http.rs`](../crates/cursdel-license/src/http.rs)
> implements the HTTP part, behind the
> [`LicenseService`](../crates/cursdel-license/src/service.rs) trait, so a
> change here should not reach the rest of the application.
>
> The **file formats** (envelope and claims) are less provisional: they
> are what offline licence validation depends on, and licences issued in
> them must keep validating for the life of their major version.

Architecture: [LICENSING-ARCHITECTURE.md](LICENSING-ARCHITECTURE.md).
Implementation decisions: [ADR-0010](adr/0010-licensing-v2.md).

## 1. Signed envelope

Licences and Business activation certificates share one envelope:

```json
{
  "format": "repass-license-v1",
  "payload": "<base64url(UTF-8 JSON claims)>",
  "signatures": [
    { "signatureAlgorithm": "Ed25519",   "keyId": "repass-signing-alpha", "signature": "<base64url>" },
    { "signatureAlgorithm": "ML-DSA-65", "keyId": "repass-mldsa65-2026",  "signature": "<base64url>" }
  ]
}
```

| Field | Rule |
|---|---|
| `format` | `repass-license-v1` for licences, `repass-activation-v1` for activation certificates. Anything else is rejected. |
| `payload` | base64url (RFC 4648 §5, **no padding**) of the claims JSON bytes. |
| `signatures` | 1–8 entries, each `keyId` at most once. No other envelope fields are allowed. |
| `signatureAlgorithm` | `Ed25519`, `ECDSA-P256-SHA256`, or `ML-DSA-65` (exact, case-sensitive). |
| `signature` | base64url, no padding. |

**Signed bytes.** Every signature covers exactly:

```
UTF8(format) || 0x0A || payload_bytes
```

where `payload_bytes` is the decoded `payload`. The server signs those
bytes as-is; nothing is canonicalised or re-serialised. The `format`
prefix stops a licence signature from being reused as an activation
certificate.

**Per-algorithm encoding.**

| Algorithm | Signature | Public key (as published to applications) |
|---|---|---|
| Ed25519 (RFC 8032) | 64 bytes | 32 bytes |
| ECDSA-P256-SHA256 (FIPS 186-5) | 64 bytes, IEEE P1363 `r‖s` (not DER); SHA-256 of the signed bytes | SEC1 point, compressed (33) or uncompressed (65) |
| ML-DSA-65 (FIPS 204) | 3309 bytes; pure ML-DSA, **empty context string** | 1952 bytes |

**How applications verify** (signatures are tried in any order):

- Skipped: unknown algorithm, algorithm not on the application's
  allowlist, or a `keyId` the build does not trust.
- Rejected: an entry whose `keyId` *is* trusted but whose
  `signatureAlgorithm` differs from that key's algorithm, or a trusted,
  allowed signature that fails to verify. This rejects the whole
  envelope, even if another signature is good.
- Accepted: at least one trusted, allowed signature verifies.

So the server may add signatures (e.g. dual Ed25519 + ML-DSA-65 during a
migration) without breaking builds that only know one of the keys. It
must only issue a licence that at least one signature lets the intended
application builds verify (architecture §6.6).

## 2. Licence claims (`schemaVersion: 1`)

```json
{
  "schemaVersion": 1,
  "licenseId": "lic_01J...",
  "customerId": "cus_01J...",
  "productId": "cursedelete",
  "majorVersion": 2,
  "edition": "business",
  "organisation": "Example Organisation Pty Ltd",
  "customerName": "Jane Citizen",
  "issuedAt": "2026-10-11T00:00:00Z",
  "perpetual": false,
  "expiresAt": "2027-10-11T00:00:00Z",
  "maxActivations": 3
}
```

| Field | Required | Rule |
|---|---|---|
| `schemaVersion` | yes | `1`. A different number is reported as unsupported. |
| `licenseId`, `customerId`, `productId` | yes | Non-empty strings. Use `[A-Za-z0-9_.-]` for ids: they appear in URL paths. |
| `majorVersion` | yes | Non-negative integer; must equal the application's major version. |
| `edition` | yes | `community`, `education`, `business`, `enterprise` (exact). |
| `organisation`, `customerName` | no | Non-empty when present. No email addresses. |
| `issuedAt` | yes | RFC 3339 with explicit offset. |
| `perpetual` | yes | `true` ⇒ `expiresAt` absent/null; `false` ⇒ `expiresAt` required and after `issuedAt`. |
| `expiresAt` | see above | Only Business may be non-perpetual. |
| `maxActivations` | Business only | Integer ≥ 1. Must be absent for every other edition. |

Unknown fields are rejected. Adding a field means a new `schemaVersion`.

## 3. Activation certificate claims (`schemaVersion: 1`)

```json
{
  "schemaVersion": 1,
  "activationId": "act_01J...",
  "licenseId": "lic_01J...",
  "productId": "cursedelete",
  "majorVersion": 2,
  "deviceId": "6F1C...E2",
  "issuedAt": "2026-10-11T00:00:00Z",
  "expiresAt": null
}
```

`deviceId` is the value the application sent at activation (64 hex
chars for CurseDelete, compared case-insensitively). `expiresAt` is
optional; when set, the certificate stops being accepted at that time and
the application must `renew`. A certificate never extends its licence:
the licence's own `expiresAt` is always checked separately.

## 4. HTTP API

Base URL: `CURSDEL_LICENSE_SERVER_URL` at runtime, or the same variable
set at build time. It must be `https://`; plain `http://` is accepted
only for `localhost`, `127.0.0.1` and `[::1]`. All bodies are JSON;
timeout 30 s.

### `POST /v1/activations`: activate / provision

```json
{
  "productId": "cursedelete",
  "majorVersion": 2,
  "activationKey": "<key the customer was given>",
  "deviceId": "<64 hex>",
  "clientSecret": "<base64url, 32 random bytes>"
}
```

- Business: allocate a seat, or re-use the existing activation for the
  same `deviceId` + licence. Enforce the seat limit transactionally.
  Store a hash of `clientSecret` for later renew/deactivate calls.
- Community / Education / Enterprise: return the licence. Do not record
  a device; ignore `deviceId` and `clientSecret`.

Response `200`:

```json
{ "license": { /* envelope */ }, "activation": { /* envelope */ } }
```

`activation` is `null` for editions without device activation.

### `POST /v1/licenses/{licenseId}/renew`: current entitlement

```json
{
  "productId": "cursedelete",
  "majorVersion": 2,
  "activation": { "activationId": "act_...", "deviceId": "...", "clientSecret": "..." }
}
```

`activation` is omitted for licences without device activation.
Response: same shape as activation. `activation` may be `null` if the
existing certificate is still current.

### `POST /v1/activations/{activationId}/deactivate`: release a seat

```json
{ "productId": "cursedelete", "majorVersion": 2, "deviceId": "...", "clientSecret": "..." }
```

Response `204` (or `200`). Counts towards the transfer allowance.

### Errors

Non-2xx responses carry:

```json
{ "error": "<machine-readable code>", "message": "<human-readable>" }
```

| Status / code | Application treats it as |
|---|---|
| `5xx`, `429`, network failure, timeout | **Unavailable**. Nothing local changes. |
| `error: "revoked"` | **Revoked**. Reported; the local licence is left alone (it already is what it is). |
| `error: "activation_inactive"` on deactivate | The seat was already released (e.g. via the portal). Local activation is removed. |
| any other `4xx` | **Rejected** with the code, e.g. `invalid_key`, `seat_limit`, `transfer_limit`, `product_mismatch`. |

The application validates every returned envelope (§1–§3) before
storing it. A licence or certificate it cannot validate is reported and
discarded, and nothing local changes.
