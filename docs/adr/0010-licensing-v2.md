# ADR-0010: Licensing v2 — product-agnostic, multi-algorithm, offline-first

Status: Accepted. Supersedes [ADR-0004](0004-licensing-integration.md).

## Context

ADR-0004 built CurseDelete's licensing as a byte-exact Rust port of the
proof-of-concept .NET licensing server's `software-license-v1` format:
ECDSA-P256 signatures over canonical JSON, multi-product entitlements, an
`updatesUntil` date instead of major-version entitlement, the device
binding and online lease embedded *inside* the licence, and an HTTP
client hard-wired to that server's API (defaulting to plain
`http://localhost:8080`). Validation returned only "valid or not", so
expired, wrong-product, untrusted-key and absent licences all looked the
same. Licences lived in one per-user path, and `license import` would
replace a valid licence with any validly signed file, even one for
another product.

[LICENSING-ARCHITECTURE.md](../LICENSING-ARCHITECTURE.md) now defines the
licensing model for every RePassCloud application: four editions,
per-major-version entitlement, Business-only device activation with a
separate certificate, offline-first Community/Education/Enterprise, and
a replacement server that must not be dictated by the old one. §15 adds
three signature algorithms with one algorithm per keypair and
application allowlists.

## Decisions

### 1. Split mechanism from policy

- **`cursdel-license`** is product-agnostic: envelope, algorithms, trusted
  keys, claims, `LicensePolicy` validation, resolution, storage
  locations, device id, and the `LicenseService` boundary. It contains
  no CurseDelete names, so another application can reuse it unchanged.
- **`cursdel-policy`** is CurseDelete's configuration: product id, major
  version (from Cargo), trusted keys, allowed algorithms, storage names,
  device-id namespace, server endpoint, Business offline behaviour, and
  the edition → feature table.
- **`cursdel-cli`** consumes both. The feature check is one call:
  `Feature::CloseRemoteLocks.is_enabled(edition)`.

### 2. Envelope with detached, multi-algorithm signatures

The payload is carried as base64url bytes and signed as
`format || "\n" || payload`. No canonical JSON is needed on either side,
so the server can be written in any language without a serialisation
interop risk (the old 231-line canonicaliser is gone), and the `format`
prefix gives domain separation between licences and activation
certificates.

`signatures` is an array, each entry naming `signatureAlgorithm` and
`keyId`. Ed25519 (`ed25519-dalek`, `verify_strict`), ECDSA-P256-SHA256
(`p256`, IEEE P1363 encoding), and ML-DSA-65 (`ml-dsa`, pure mode, empty
context) are supported. A trusted key is bound to one algorithm. An
entry whose claimed algorithm differs from its key's is rejected, as is
any trusted, allowed signature that fails. Unusable entries (unknown
algorithm, disallowed algorithm, untrusted key) are skipped, which is
what makes dual-signing a safe migration path: an older build that only
trusts the Ed25519 key accepts a licence that a newer build verifies via
ML-DSA-65.

Choosing ECDSA-P256 does not make CurseDelete FIPS-compliant: the
RustCrypto implementations are not FIPS-validated modules. A build that
must accept only FIPS-approved algorithms narrows
`cursdel_policy::ALLOWED_ALGORITHMS`; real FIPS compliance would also need a
validated module and environment (architecture §15.3).

`ml-dsa` 0.1 requires Rust 1.85, so the workspace `rust-version` moved
from 1.82 to 1.85.

### 3. One product, one major version, one edition per licence

Claims carry `productId`, `majorVersion` and `edition` directly. There
are no multi-product licences: bundles are resolved server-side into
separate licences (architecture §8.3). Edition invariants are enforced
at parse time: Community/Education/Enterprise are always perpetual, and
only Business carries `maxActivations`. Unknown fields are rejected, so
a future restrictive claim can never be silently ignored; it needs a new
`schemaVersion`, which older builds report as unsupported.

### 4. Business activation is a separate certificate

The licence says what was bought; the `repass-activation-v1` certificate
says the server authorised this device. Both are validated, and the
licence's own term is always checked, so an old certificate cannot
extend an expired subscription. CurseDelete sets
`business_offline_limit: None`: activate online once, then work offline
indefinitely. Revocation through the portal therefore reaches a device
only at its next successful `renew`, and the docs say exactly that
rather than promising remote revocation. Another application can set a
limit to force periodic renewal.

The device id is the existing SHA-256-of-OS-machine-id scheme, now
namespaced per application, with the hostname no longer sent.
Community/Education/Enterprise never compute it.

### 5. Detailed, non-fatal resolution

`resolve()` validates every installed candidate and returns the active
licence (the highest valid edition) plus every rejected candidate with a
`Rejection` reason: malformed, unsupported format/schema/algorithm,
disallowed algorithm, untrusted key, invalid signature, wrong product,
wrong major version, expired, activation required, or activation
invalid. Revoked and server-unavailable are `ServiceError`s from online
calls, because a local, offline check cannot know them. Nothing here
stops deletion; the effective edition just falls back to Community.

### 6. Per-major-version storage with an administrator directory

Licences are looked up, in precedence order, from `CURSDEL_LICENSE_FILE`,
the per-user `…/v2/license.json`, and every `*.license`/`*.json` file in
the administrator directory (`/etc/cursedelete/v2`,
`/Library/Application Support/CurseDelete/v2`,
`%ProgramData%\RePassCloud\CurseDelete\v2`, or `CURSDEL_LICENSE_DIR`).
Per-major directories let v2 and v3 coexist on one machine. Writes are
atomic (temp + rename). The client secret is `0600`. Nothing invalid is
ever written, and a failed network call never removes or replaces local
state.

### 7. Server boundary, no fake server

`LicenseService` (activate / renew / deactivate) isolates the network.
`HttpLicenseService` implements a contract documented as **provisional**
in [LICENSING-SERVER-CONTRACT.md](../LICENSING-SERVER-CONTRACT.md),
requires `https://` (loopback `http://` excepted), and has no default
URL. With no URL configured, `UnconfiguredService` makes online commands
fail with a clear message while offline licensing keeps working. Tests
use an in-process fake and deterministic, publicly-known test keys
(`cursdel_license::testing`, behind the `test-support` feature); no
temporary server was invented.

### 8. The production trust set ships empty

No production signing keys exist yet, and per the architecture they are
never generated in an application repository. Until the licensing server
publishes keys and they are added to `cursdel_policy::TRUSTED_KEYS`,
every licence is rejected as untrusted and CurseDelete runs as
Community. The trust set is compile-time only. There is deliberately no
environment variable or file that adds trusted keys at runtime, because
that would let anyone mint their own licences.

## Consequences and migration

- **Old licences stop working.** `software-license-v1` / ECDSA-P256-over-
  canonical-JSON files and the P-256 keys `primary-2026` /
  `secondary-2026` are no longer accepted. Such a file is reported by
  `license status` as an unusable licence file and CurseDelete runs as
  Community, which is never a crash. v2.0.0 shipped with the old format,
  but its signing server was a proof of concept, so this change assumes
  no customer licences were issued in it and keeps no compatibility
  shim. **If any were, they need reissuing in the new format, or a
  verify-only legacy path has to be added before release.** That is an
  open release decision.
- **Versioning.** The `license` CLI changes below break scripts written
  against 2.0.0's syntax. Shipping this as 2.x keeps licences tied to
  major version 2 (none have been issued in the new format yet), at the
  cost of a CLI break inside a major version; shipping it as 3.0 would
  make every licence a v3 licence. Also an open release decision.
- **CLI changes.** `activate --license-id … --activation-code …` became
  `activate <KEY>`. `activate --offline` / `--output` are gone: Business
  needs online activation (architecture §4.1), and every other edition
  installs a file offline. `import` is now `install` (`import` remains
  an alias), and `refresh` is now `renew` (alias kept). `status --json`
  is new.
- **Storage moved** from `…/CurseDelete/license.json` +
  `activation.json` to `…/CurseDelete/v2/`. Old files are ignored.
- **Edition strings are exact.** The old `smb`/`project`/`corporate`/
  `consumer` aliases are gone; pricing bands are a server/Stripe concern.
- **MSRV** is now Rust 1.85.
- `deny.toml` needed no new licence allowances: `ed25519-dalek`/
  `curve25519-dalek` are BSD-3-Clause, and `ml-dsa`/`p256` are MIT OR
  Apache-2.0.
