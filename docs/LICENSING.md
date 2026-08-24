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

CurseDelete has two storage **scopes**:

- **User** (per-account) — written by `license activate`/`license import`.
- **Machine** (shared by every account on the machine) — written by
  `license enroll` (a Deployment Key).

Each scope holds the same two files: `license.json` (the signed license
envelope) and `activation.json` (activation ID + bearer token, for online
activations only). The activation token is a bearer credential — treat it
like a password. It's never stored in an OS keychain: keychain access on
some platforms (notably macOS Keychain for an unsigned/ad-hoc-signed
binary) can require interactive GUI consent, which would break
unattended/CI/scheduled automation — a requirement the Business/Enterprise
editions explicitly need to support. This mirrors the default behavior of
other automation-friendly CLIs in the same space (`aws configure`,
`gcloud auth`, `gh auth login`). See
[ADR-0004](adr/0004-licensing-integration.md#credential-storage-protected-file-not-an-os-keychain)
for the full reasoning.

### User scope

| Platform | Location |
|---|---|
| Windows | `%LOCALAPPDATA%\RePassCloud\CurseDelete-2\` |
| Linux | `~/.config/cursedelete-2/` (XDG `config` dir, honors `XDG_CONFIG_HOME`) |
| macOS | `~/Library/Application Support/CurseDelete-2/` |

`activation.json` is created with owner-only permissions (`0600` on
Unix); on Windows it relies on the user profile's own owner-restricted
NTFS ACL inheritance.

### Machine scope

| Platform | Location |
|---|---|
| Windows | `%PROGRAMDATA%\CurseDelete-2\` |
| Linux | `/var/lib/cursedelete-2/` |
| macOS | `/Library/Application Support/CurseDelete-2/` |

`license.json` is world-readable (`0644` on Unix; on Windows it inherits
`%ProgramData%`'s normal Users-readable ACL) — any local account running
`cursdel` needs to resolve the machine's license. `activation.json` — the
bearer credential — is restricted to the owner/root on Unix (`0600`) and
to `BUILTIN\Administrators`/`SYSTEM` on Windows (an explicit `icacls`
grant applied at write time, since `%ProgramData%` is otherwise readable
by every local account, unlike the user-scope path above).

### Resolution order: machine → user → Community

Every command that needs the active license (`status`, ordinary deletion
resolving `--close-remote-locks`, etc.) checks **machine scope first,
then user scope, then falls back to Community**. The first *present and
valid* license wins.

An invalid machine-scope license is **not** silently bypassed in favor of
a valid user-scope one — `license status` reports the broken machine
license explicitly rather than quietly falling through, so a revoked or
corrupted machine-wide enrollment is never masked by whatever older
per-user license happens to still be sitting on disk. (Ordinary deletion
still fails open to Community rather than crashing, per this project's
existing "an invalid license must never break deletion" policy — it just
does so having reported the problem via `license status`, not by silently
preferring a different scope.)

`refresh` and `deactivate` operate on whichever scope actually has
activation credentials present (machine before user), and never mix a
license from one scope with credentials from the other.

Named `CurseDelete-2` (rather than plain `CurseDelete`) so this crate's
license state can never collide with the separate, pre-rewrite C#
CurseDelete implementation.

## `CURSDEL_LICENSE_SERVER_URL`

Sets the base URL of the license server for online activation, refresh,
enrollment, and deactivate calls. Defaults to the RePass Cloud-operated
production server, `https://license-server.repasscloud.com`, when unset —
production use needs no configuration. Set this variable to point at a
development or self-hosted server instead:

```bash
export CURSDEL_LICENSE_SERVER_URL=http://localhost:8080
cursdel license activate --license-id LIC-... --activation-code ...
```

This variable is irrelevant to `license status`, `license import`, and
ordinary deletion — those never make a network call.

## Unattended enrollment with a Deployment Key

A **Deployment Key** is a separate credential from manual activation's
License ID + Activation Code. It is not a license itself — it is an
enrollment credential, created ahead of time by a license administrator
against a specific license, that lets unattended tooling (Intune, an RMM,
a golden VM image, a container entrypoint script) enroll a machine without
a human typing an activation code:

```text
Manual activation:
    License ID + Activation Code

Deployment enrollment:
    Deployment Key
```

Both produce the same kind of machine-bound activation under the hood —
enrollment is just a second entrypoint into it, purpose-built for
automation.

```console
$ cursdel license enroll --deployment-key-env CURSDEL_DEPLOYMENT_KEY
Machine enrolled successfully via Deployment Key.
License ID:   LIC-ABC123
Activation:   3f2a9c1e-...-guid
Scope:        machine-wide
```

Exactly one of the following supplies the key. Prefer the non-CLI-argument
forms in automated environments, since process command-line arguments can
be visible to other local accounts or recorded in shell history/process
logs:

| Flag | Use |
|---|---|
| `--deployment-key <VALUE>` | Direct value; convenient for interactive/testing use. |
| `--deployment-key-env <ENV_VAR>` | Reads the key from the named environment variable. |
| `--deployment-key-file <PATH>` | Reads the key from a file's contents. |
| `--deployment-key-stdin` | Reads one line from standard input. |

The Deployment Key is **never** written to disk, printed, or logged by
`cursdel` — not in `activation.json`, not in normal output, not in error
messages. Once enrollment succeeds, the resulting `license.json` and
`activation.json` behave exactly like a manually-activated machine's:
`license refresh` and `license deactivate` use the stored activation
credentials, never the Deployment Key. This means revoking a Deployment
Key stops *future* enrollments without breaking machines already enrolled
through it.

Re-running `license enroll` on an already-enrolled, still-valid machine is
safe — it reports success without contacting the server or consuming
another seat. If the license's seat pool is exhausted, enrollment fails
clearly and does **not** fall back to Community or a lower license tier:

```console
$ cursdel license enroll --deployment-key-env CURSDEL_DEPLOYMENT_KEY
Error: licence activation limit reached (500/500 active seats).
Contact your licence administrator or RePass Cloud support.
```

### Machine-wide storage

Deployment Key enrollment writes to the machine scope described in
["Where credentials and license files are stored"](#where-credentials-and-license-files-are-stored)
above (`%PROGRAMDATA%\CurseDelete-2\` / `/var/lib/cursedelete-2/` /
`/Library/Application Support/CurseDelete-2/`), never the per-user
location manual `activate`/`import` use.

This requires the process to have administrator/root privileges (or
equivalent write access to that path). Enrollment fails with a clear error
— never a silent downgrade to per-user or Community — if it can't
establish machine-wide state:

```console
$ cursdel license enroll --deployment-key-env CURSDEL_DEPLOYMENT_KEY
Error: cannot write machine-wide licence state at /var/lib/cursedelete-2/license.json: Permission denied (os error 13)
Re-run with administrator/root privileges.
```

`license status` reports which scope is active (`machine-wide (Deployment
Key enrollment)` or `user`) and the resolved license's path when a valid
license is found; machine scope always takes precedence over user scope
per the [resolution order](#resolution-order-machine--user--community)
above — including reporting, not silently ignoring, an invalid
machine-scope license rather than falling through to a user-scope one.
