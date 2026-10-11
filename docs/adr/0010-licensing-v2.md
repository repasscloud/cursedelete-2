# ADR-0010: Licensing v2 — product-agnostic, multi-algorithm, offline-first

Status: Accepted (CurseDelete 2.1.0). Supersedes [ADR-0004](0004-licensing-integration.md).

## Context

ADR-0004 built CurseDelete's licensing as a byte-exact Rust port of the
proof-of-concept .NET licensing server's `software-license-v1` format:
ECDSA-P256 signatures over canonical JSON, multi-product entitlements, an
`updatesUntil` date instead of major-version entitlement, the device
binding and online lease embedded *inside* the licence, and an HTTP
client hard-wired to that server's API (defaulting to plain
`http://localhost:8080`). Validation returned only "valid or not", so
expired, wrong-product, untrusted-key and absent licences all looked the
same. Licences lived in one per-user path, `license import` would
replace a valid licence with any validly signed file, and the Windows
installers invoked a `license enroll` command that did not exist.

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
indefinitely. A portal release therefore reaches a device at its next
`renew` (the server answers `activation_inactive` and the client removes
the local activation), and the docs say exactly that rather than
promising remote revocation. Another application can set a limit to
force periodic renewal.

The device id is the existing SHA-256-of-OS-machine-id scheme, now
namespaced per application, with the hostname no longer sent.
Community/Education/Enterprise never compute it.

### 5. Detailed, non-fatal resolution

`resolve()` validates every installed candidate and returns the active
licence (the highest valid edition) plus every rejected candidate with a
`Rejection` reason: malformed, unsupported format/schema/algorithm,
disallowed algorithm, untrusted key, invalid signature, wrong product,
wrong major version, expired, revoked, activation required, or
activation invalid. "Server unavailable" is a `ServiceError` from online
calls. Nothing here stops deletion; the effective edition just falls
back to Community.

**Revocation, when known** (architecture §5.7, §9.4): when `renew` gets
`revoked`, the licence id is recorded in the scope's `revocations.json`,
and resolution rejects that licence from then on. Administrators can
list licence ids there too. An unsigned list is safe to honour because
it can only remove entitlement. A later successful renewal or activation
of the same licence clears the record.

### 6. Per-major-version storage in two scopes

All licence state lives in per-major-version directories (`…/v2/`), so v2
and v3 coexist on one machine. There are two writable scopes:

- **User** (default): `%LOCALAPPDATA%\RePassCloud\CurseDelete\v2`,
  `~/Library/Application Support/CurseDelete/v2`, or
  `~/.config/cursedelete/v2`.
- **System** (`--system`, for installers and site-wide deployment):
  `%ProgramData%\RePassCloud\CurseDelete\v2`,
  `/Library/Application Support/CurseDelete/v2`, or `/etc/cursedelete/v2`
  (or `CURSDEL_LICENSE_DIR`). Administrators may also drop `*.license` /
  `*.json` files here by hand.

Licences are read, in precedence order, from `--license-file` /
`CURSDEL_LICENSE_FILE`, the user licence, then the system directory.
Activation certificates and revocations are read from both scopes.
Writes are atomic (temp + rename). The client secret is `0600` on Unix;
on Windows a system-scope secret is restricted to SYSTEM and
Administrators with `icacls`. Nothing invalid is ever written, and a
failed network call never removes or replaces local state.

The Windows installers take `/LICENSEFILE=` / `/ACTIVATIONKEY=` (Inno)
and `LICENSEFILE` / `ACTIVATIONKEY` (MSI), running `license install
--system` / `license activate --system`. A licensing problem never fails
the install. This replaces the non-existent `license enroll
--deploymentkey` call and the "grant Users Modify on `*.json` in Program
Files" step, since nothing is written to the installation directory any
more.

### 7. Server boundary and handoff

`LicenseService` (activate / renew / deactivate) isolates the network.
`HttpLicenseService` implements the API in
[LICENSING-SERVER-BRIEF.md](../LICENSING-SERVER-BRIEF.md). That document
is the specification the replacement server is built against and is
self-contained. It requires `https://` (loopback `http://` excepted) and
has no default URL: the release workflow can bake one in from the
`CURSDEL_LICENSE_SERVER_URL` repository variable. With no URL,
`UnconfiguredService` makes online commands fail with a clear message
while offline licensing keeps working.

Tests use an in-process fake and deterministic, publicly-known test keys
(`cursdel_license::testing`, behind the `test-support` feature). The
brief embeds deterministic test vectors for all three algorithms, which
a unit test regenerates and verifies so they can't drift. The
`conformance` example lets the server team validate their output with
the real client code.

### 8. Trusted keys are public keys in source

The licensing server owns every private key. Public keys are added to
`cursdel_policy::TRUSTED_KEYS` as `(keyId, algorithm, base64url key)`
when the server publishes them, and kept for as long as licences they
signed must keep validating in this major version. The trust set is
compile-time only. There is deliberately no environment variable or file
that adds trusted keys at runtime, because that would let anyone mint
their own licences. The list is empty until the new server generates
its first keys; until then every licence is reported as signed by an
untrusted key, and CurseDelete runs as Community.

## Consequences

- Shipped as **2.1.0**. 2.0.0's `software-license-v1` licences were test
  licences only and are not supported. Such a file shows in
  `license status` as a licence file not in use.
- **CLI:** `license status [--json] | install | activate <KEY> | deactivate |
  renew`, with `--system` on the write commands and a global
  `--license-file`. `import`/`refresh` remain as aliases. The 2.0.0 flags
  `activate --license-id/--activation-code/--offline/--output` are gone:
  Business needs online activation (architecture §4.1), and every other
  edition installs a file offline.
- **Edition strings are exact.** The old `smb`/`project`/`corporate`/
  `consumer` aliases are gone; pricing bands are a server/Stripe concern.
- **MSRV** is Rust 1.85.
- `deny.toml` needed no new licence allowances: `ed25519-dalek`/
  `curve25519-dalek` are BSD-3-Clause, and `ml-dsa`/`p256` are MIT OR
  Apache-2.0.
