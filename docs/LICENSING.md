# Licensing

CurseDelete's core deletion engine is identical in every edition — adaptive
and manual worker control, `--force` remediation, retention, filters,
dry-run, and JSON output all run at full speed regardless of license.
Community is free and is not artificially slowed down. What editions
actually differ on is commercial-use rights, scale, automation posture, and
one specific technical capability. This document covers the edition model,
exactly what is and isn't technically enforced, the `license` subcommand,
and where credentials are stored.

## Editions

| Edition | Price | Who it's for |
|---|---|---|
| Community | Free | Personal use, developers, homelabs, evaluation |
| Education | Free | Students, teachers, recognized schools/universities, classroom/lab and non-commercial academic use |
| Business | Commercial | IT departments, DevOps teams, MSPs, file-server admins, build/release infrastructure |
| Enterprise | Custom/high-tier commercial | Organization-wide deployment, negotiated scale, priority support |

CurseDelete's licensing model has more optional pricing-band SKUs
(`project`, `smb`, `corporate`, `consumer`) in the underlying license
schema for commercial flexibility, but they all map onto one of the four
technical tiers above — see [`Edition::from_license_str`](../crates/cursdel-policy/src/lib.rs)
for the exact mapping (`smb`/`project` → Business; `corporate` →
Enterprise; `consumer` → Community). An edition string the client doesn't
recognize at all fails closed to Community — it is never treated as
granting more than the free tier.

## What is actually technically gated

This is the important distinction: most of what differs between editions
above is a **licensing-terms** matter (what your purchase legally entitles
you to do), not something `cursdel` enforces at runtime. Enforcing
commercial-use restrictions, deployment scale, or automation rights
technically would make the tool worse for everyone without meaningfully
protecting anything — per
[`cursdel-policy`](../crates/cursdel-policy/src/lib.rs)'s own module
documentation, the deletion engine never consumes this crate and never
will.

**The one capability that is technically gated:**

- **`--close-remote-locks`** — administratively closing a remote SMB open
  on a supported Windows file server. Rejected outright (exit code `7`,
  `LicenseRequired`) without a currently-valid Business or Enterprise
  entitlement. See [LOCKS.md](LOCKS.md#--close-remote-locks-remote-smb-opens)
  for why this specific capability, and only this one, is unambiguously
  reserved.

Everything else in the edition matrix — commercial use, CI/build-agent
automation rights, organizational deployment, priority support — is a
legal/licensing-terms distinction your purchase agreement covers, not a
runtime check `cursdel` performs. `--kill-locks`, `--force`/`--destroy`
remediation, retention, and filters all work identically regardless of
license state.

Absent, missing, or invalid licensing always resolves to Community
capabilities — CurseDelete fails **open** to the free tier, never closed to
nothing. A corrupted or expired license file is reported clearly and the
tool keeps working at the Community level; it is never a hard failure.

## The `license` subcommand

See [COMMAND_REFERENCE.md](COMMAND_REFERENCE.md#license-subcommand) for the
full flag reference. Summary of the flow:

### `cursdel license status`

Shows the current license/activation state, or confirms you're running on
Community capabilities with no license present.

```console
$ cursdel license status
CurseDelete License Status

No active license on this device -- running with Community capabilities.

Activate with:
  cursdel license activate --license-id <id> --activation-code <code>
```

### `cursdel license activate --license-id <ID> --activation-code <CODE>`

Online activation: contacts the license server directly, using the License
ID and Activation Code from your purchase email, and — on success —
persists the signed license envelope and activation credentials to disk.
Requires network access to the license server.

### `cursdel license activate --license-id <ID> --activation-code <CODE> --offline [--output <PATH>]`

For air-gapped machines with no path to the license server. Writes an
activation request file (default `./offline-activation-request.json`)
instead of contacting the server:

```console
$ cursdel license activate --license-id LIC-DEMO123 --activation-code ABC-DEF-GHI --offline
Offline activation request written to offline-activation-request.json

Email this file to hello@repasscloud.com.
Once you receive a signed license file back, run:
  cursdel license import <path-to-license-file>
```

The written request contains a randomly-generated `requestId` and
`activationToken`, plus this device's identity — no plaintext secret beyond
the activation code you already typed. Email the file to
**hello@repasscloud.com**; an admin with network access activates on your
behalf, and relays a signed license file back for you to import.

### `cursdel license import <PATH>`

Imports a signed license file received back from an offline activation
request (or any other admin-issued license file, such as a hand-signed
education/enterprise grant). Verifies the file's signature and schema
before saving it — a file that fails verification is rejected with a clear
error and never partially applied.

### `cursdel license deactivate`

Frees this device's activation, so the license can be activated on another
device. A given license only allows one active activation at a time —
activating elsewhere while one is already active is rejected until the
existing one is deactivated first.

### `cursdel license refresh`

Renews the current online activation lease before it expires. Online
activations carry a lease (`refreshAfter`/`leaseExpiresAt`) that must be
periodically renewed — this is a deliberate "phone home periodically"
mechanic to keep the license current, not a bug. Offline-mode activations
have no lease and this command reports that plainly rather than attempting
a network call:

```console
$ cursdel license refresh
Error: this device was activated offline, which has no lease to refresh.
Offline licenses do not need periodic refresh.
```

## How verification works (no network required to use an existing license)

Once a signed license file is on disk, `cursdel` verifies it locally on
every invocation — no network call, no dependency on the license server
being reachable. Verification is a from-scratch Rust reimplementation of
the same `software-license-v1` ECDSA-P256-SHA256 signing scheme the
existing .NET license server (`Licensing.Core`) uses, validated
byte-for-byte against real license files signed by the actual
`LicenseGenerator` tool — not merely a self-consistent Rust round-trip. See
[ADR-0004](adr/0004-licensing-integration.md) for the full architecture,
including why a native reimplementation was chosen over an FFI bridge or
sidecar process, and [LICENSING-INTEGRATION.md](../LICENSING-INTEGRATION.md)
for the underlying protocol this implementation targets.

A network call is only ever made for: online activation, `license refresh`,
and `license deactivate`. A previously-verified, still-valid license
continues to work with zero network dependency.

## Where credentials and license files are stored

The activation token is a bearer credential — treat it like a password.
It's stored in a file with owner-only permissions (`0600` on Unix) rather
than an OS keychain: keychain access on some platforms (notably macOS
Keychain for an unsigned/ad-hoc-signed binary) can require interactive
GUI consent, which would break unattended/CI/scheduled automation — a
requirement the Business/Enterprise editions explicitly need to support.
This mirrors the default behavior of other automation-friendly CLIs in the
same space (`aws configure`, `gcloud auth`, `gh auth login`). See
[ADR-0004](adr/0004-licensing-integration.md#credential-storage-protected-file-not-an-os-keychain)
for the full reasoning.

| Platform | Location |
|---|---|
| Windows | `%LOCALAPPDATA%\RePassCloud\CurseDelete\` |
| Linux | `~/.config/cursedelete/` (XDG `config` dir, honors `XDG_CONFIG_HOME`) |
| macOS | `~/Library/Application Support/CurseDelete/` |

Two files live in that directory: `license.json` (the signed license
envelope) and `activation.json` (activation ID + bearer token, for online
activations only).

## `CURSDEL_LICENSE_SERVER_URL`

Sets the base URL of the license server for online activation, refresh,
and deactivate calls. Defaults to `http://localhost:8080` (the documented
development URL) when unset — there is no hardcoded production hostname,
so a real deployment supplies its production URL entirely through this
environment variable:

```bash
export CURSDEL_LICENSE_SERVER_URL=https://license.example.com
cursdel license activate --license-id LIC-... --activation-code ...
```

This variable is irrelevant to `license status`, `license import`, and
ordinary deletion — those never make a network call.
