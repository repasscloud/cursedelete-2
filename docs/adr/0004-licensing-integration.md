# ADR-0004: Licensing integration architecture

## Status

Accepted.

## Context

The existing licensing ecosystem (`danijeljw-RPC/licsense-server-poc`) ships
`Licensing.Core` as a .NET 10 NuGet package: offline, no-network signature
verification of a signed JSON "licence envelope" (`software-license-v1`,
ECDSA-P256-SHA256). CurseDelete is Rust. `LICENSING-INTEGRATION.md`
(section 11) explicitly leaves the resolution of this mismatch to the
implementer, while requiring: product code `cursedelete`; preserved
compatibility with the existing server and envelope format; preserved
ECDSA verification semantics; preserved edition/expiry/activation/device-
binding behaviour; preserved online activation, offline activation,
refresh, deactivate; no private signing material copied into CurseDelete;
no network dependency to verify a previously-issued offline licence.

Architectures considered:

1. **Native Rust reimplementation of the verifier** (chosen). The signing
   scheme is fully specified and small: canonical JSON serialisation +
   ECDSA-P256-SHA256 with IEEE P1363 (raw `r‖s`) signatures over the
   canonical bytes, verified against embedded public keys. Reimplementing
   it in Rust needs no runtime dependency on .NET, no IPC, no process
   boundary, and -- because only *public* keys are embedded -- introduces
   no new secret-handling surface.
2. **NativeAOT bridge**: compile `Licensing.Core` to a native library via
   .NET NativeAOT and call it via FFI. Rejected: adds a second toolchain
   (.NET SDK) to every CurseDelete build, a NativeAOT compatibility burden
   for a small, stable, already-fully-specified algorithm, and a
   nontrivial FFI surface (marshalling JSON strings and structured error
   results across the boundary) for something that reimplements to a few
   hundred lines of Rust.
3. **Sidecar process**: ship a small .NET executable CurseDelete shells out
   to for verification. Rejected for the same reasons as (2), plus adding
   process-spawn latency to a path (`cursdel` startup) that runs on every
   invocation, and a second binary to package/sign/distribute per platform.

Reimplementation is only safe if it is **byte-exact** with the C#
implementation, since the ECDSA signature covers the canonical JSON bytes
directly -- any divergence in serialisation and every real licence fails to
verify. The highest-risk unknown was .NET's `Utf8JsonWriter` *default*
string-escaping behaviour, which is materially more aggressive than bare
JSON requires (see below). Rather than guess at it from documentation,
this was resolved empirically: the real `LicenseGenerator` CLI from
`licsense-server-poc` was run locally (`dotnet run --project
src/LicenseGenerator -- keygen` / `sign`) against a disposable, throwaway
ECDSA keypair generated solely for this purpose (never a production key,
and the private key was discarded immediately after generating the test
fixtures) to sign a licence payload containing every printable ASCII
character, every standard control-character escape, and a sample of non-
ASCII/astral Unicode. The resulting signed file
(`crates/cursdel-license/tests/fixtures/exhaustive.license`) pins the
escaping table down exactly; `crates/cursdel-license/src/canonical_json.rs`
implements it and is tested directly against that fixture
(`matches_real_dotnet_output_exhaustive`), plus two more real
C#-signed fixtures whose signatures the Rust verifier must and does
successfully verify end-to-end (`verifies_real_dotnet_signed_license`,
`verifies_exhaustive_character_coverage_license`).

The escaping rules (see `canonical_json.rs` module docs for the full
table): `"` is written as `"`, not the JSON-shorthand `\"`; `&`, `'`,
`+`, `<`, `>`, and `` ` `` are all escaped as `\uXXXX` even though bare
JSON does not require it (`Utf8JsonWriter`'s default encoder is
conservative against HTML/script injection); everything outside printable
ASCII is escaped as UTF-16 code units (`\uXXXX`, surrogate pairs for
astral characters), all with **uppercase** hex digits.

## Decision

Implement `cursdel-license` as a from-scratch, dependency-minimal Rust
crate:

- `canonical_json.rs` -- byte-exact reimplementation of
  `CanonicalJson.cs`, validated against real C#-signed fixtures.
- `schema.rs` -- byte-exact reimplementation of `LicenseSchema.cs`'s
  strict validation rules (unknown-field rejection, case-insensitive
  duplicate-field rejection, `deviceBinding`/`activation` both-or-neither,
  non-empty unique-product `entitlements`, mandatory-offset ISO-8601
  timestamps, lowerCamelCase-only `metadata`).
- `trusted_keys.rs` -- the same two public keys (`primary-2026`,
  `secondary-2026`) from `TrustedPublicKeys.cs`, copied verbatim. These
  are public keys; embedding them is the entire point of an offline
  verifier and reveals no private material.
- `verify.rs` -- signature verification (`p256`/`ecdsa` crates, IEEE
  P1363 raw-concatenation signatures) plus `validate_product`/
  `validate_activation`, matching `LicenseVerifier.cs`'s semantics
  (perpetual licences carry a real far-future `expiresAt` rather than
  `None`, so expiry checking is uniform; device mismatch and lease
  expiry both raise a validation error with the same user-facing shape).
- `device.rs` -- local device identity. **The hash formula deliberately
  does not need byte parity with C#'s implementation.** Unlike the
  signature envelope, `deviceId` is an opaque, client-chosen value: the
  server only ever stores and echoes back whatever the activating client
  sent, it never recomputes it independently. The only interop
  requirements are the literal `scheme` string (`"os-machine-id-sha256-v1"`,
  checked by the schema) and the 64-hex-char format. This freed the
  implementation to use a **stronger** macOS identifier than the
  reference: `Licensing.Core`'s own doc comments flag its macOS behaviour
  as a weak fallback to `Environment.MachineName` (trivially user-
  changeable); this implementation instead reads `IOPlatformUUID` (via
  `ioreg`), a real hardware-tied identifier that survives a rename and
  most OS reinstalls. Windows reads the same `MachineGuid` registry value
  via `RegGetValueW`; Linux reads `/etc/machine-id`/`/var/lib/dbus/machine-id`,
  same as the reference.
- `client.rs` -- online activation/validate/refresh/deactivate HTTP calls
  matching the exact DTOs in `LICENSING-INTEGRATION.md` §6 (which are not
  shipped in `Licensing.Core` -- they live in the server's internal
  `ApiContracts.cs` and are reproduced here by the letter of that
  section), via `ureq`. Network failures are a distinct error variant
  from a rejected/invalid licence, so callers can fall back to the last-
  verified local file rather than treating "couldn't reach the server" as
  "licence invalid" (§9's explicit requirement).
- `store.rs` -- platform-conventional storage paths (see below) and
  activation-credential persistence.

### Credential storage: protected file, not an OS keychain

The activation token is a bearer credential (`LICENSING-INTEGRATION.md`
§6.1: "treat it like a password"). The obvious first instinct is an OS
keychain (macOS Keychain, Windows Credential Manager, Linux Secret
Service). This was deliberately **not** used: keychain access on some
platforms -- notably macOS Keychain for an unsigned or ad-hoc-signed
binary -- can require interactive user consent (a GUI prompt), and Linux
Secret Service requires a running desktop session/daemon that is routinely
absent on headless servers and CI runners. Both directly conflict with the
product's explicit Business/Enterprise requirement for **unattended,
scheduled, CI/build-agent automation**. Instead, the activation token is
written to a file created with owner-only permissions (`0600` via
`OpenOptions::mode` on Unix; relies on the user profile's default NTFS ACL
inheritance under `%LOCALAPPDATA%` on Windows, matching where the file
already lives). This mirrors the default behaviour of mature automation-
friendly CLIs in the same problem space (`aws configure`, `gcloud auth`,
`gh auth login` all default to a protected local file, not a keychain).

### Storage locations

Exactly the platform-conventional paths specified by the product brief,
implemented via `directories::BaseDirs` for OS-correct base directories
with the exact subpath components appended manually (not
`directories::ProjectDirs`, whose automatic naming convention would not
reproduce the asymmetric shape required -- an organisation subfolder on
Windows but not on macOS):

- Windows: `%LOCALAPPDATA%\RePassCloud\CurseDelete\`
- Linux: `~/.config/cursedelete/` (XDG, honouring `XDG_CONFIG_HOME`)
- macOS: `~/Library/Application Support/CurseDelete/`

### Server URL configuration

`CURSDEL_LICENSE_SERVER_URL` environment variable, defaulting to the
documented development URL (`http://localhost:8080`, matching
`appsettings.json`'s `PublicBaseUrl`) when unset. No production hostname
is hardcoded -- per the task's explicit instruction not to invent one --
so a real production URL can be supplied entirely through deployment
configuration (packaging, environment, or a future config file) without a
code change.

### Product code

`cursedelete`, as mandated. Every `ValidateProduct`-equivalent call in
this codebase uses this constant (`cursdel_license::PRODUCT_CODE`); it
must be registered identically on the licence server side.

## Consequences

- Verified against real C#-signed licences produced by the actual
  `LicenseGenerator` tool (not merely self-consistent Rust-to-Rust round
  trips), which is the strongest practical evidence of interop short of a
  live end-to-end run against the deployed server.
- No .NET runtime, NativeAOT toolchain, or sidecar process is required to
  build or run `cursdel`.
- If the server rotates signing keys, `trusted_keys.rs` must be updated
  deliberately (new key ID added, matching `TrustedPublicKeys.cs`) and a
  new CurseDelete release cut -- there is no mechanism to fetch trust
  updates over the network, which is intentional: it is exactly what
  keeps offline verification of a previously-issued licence free of any
  network dependency.
- **Not yet validated**: an actual end-to-end run against a live,
  deployed license server (online activation, refresh, deactivate). The
  request/response shapes are implemented exactly per
  `LICENSING-INTEGRATION.md` §6-7, and the signature/schema logic is
  proven against real fixtures, but no live server was available in this
  environment to exercise the HTTP activation flow itself. This is
  recorded as required follow-up validation before a production release.
