# Licensing

CurseDelete's deletion engine is identical in every edition. Adaptive and
manual worker control, `--force`/`--destroy` remediation, `--kill-locks`,
retention, filters, dry-run, and JSON output all run at full speed with
no licence at all. Community needs no registration, no licence file, and
no network connection. Editions differ in commercial-use rights, scale,
support, and one technical capability.

This page covers the edition model, what is actually gated, the
`license` subcommand, where licence files live, and how offline use and
revocation behave. The cross-application design is in
[LICENSING-ARCHITECTURE.md](LICENSING-ARCHITECTURE.md), and the reasoning
for this implementation is in [ADR-0010](adr/0010-licensing-v2.md).

## Editions

| Edition | Price | Who it's for | Licence | Device activation |
|---|---|---|---|---|
| Community | Free | Personal use, developers, homelabs, evaluation | None needed; optional free registration gives a registered Community licence | Never |
| Education | Free | Students, teachers, schools and universities; classroom, lab and non-commercial academic use | Institution-owned volume licence, perpetual | Never |
| Business | Commercial | IT departments, DevOps teams, MSPs, file-server admins, build/release infrastructure | Perpetual or subscription, with a seat count | Online, once per device |
| Enterprise | Custom | Organisation-wide deployment including subsidiaries | Perpetual, unlimited installations | Never |

Every licence is for **one product and one major version**. A
CurseDelete v2 licence covers every 2.x.y release, never v1 or v3.
Perpetual means the v2 entitlement never expires. It does not include
future major versions.

## What is technically gated

| Feature | Community | Education | Business | Enterprise |
|---|---|---|---|---|
| Everything except the row below | ✓ | ✓ | ✓ | ✓ |
| `--close-remote-locks` (close remote SMB opens on a Windows file server) | | | ✓ | ✓ |

A registered Community licence currently unlocks nothing extra in the
tool itself; registration is for the customer portal and update
notifications. Commercial use, organisational deployment, automation
rights and support are licensing terms, not runtime checks. The edition
→ feature table lives in one place,
[`cursdel_policy::Feature`](../crates/cursdel-policy/src/lib.rs).

`--close-remote-locks` without a valid Business or Enterprise licence
exits with code `7` (`LicenseRequired`) before touching anything.

**Licensing never stops CurseDelete working.** A missing, corrupt,
expired, wrong-version or untrusted licence means Community, and
`license status` says why.

## The `license` subcommand

```text
cursdel license status [--json]
cursdel license install <PATH> [--system]      (alias: import)
cursdel license activate <KEY> [--system]
cursdel license deactivate [--system]
cursdel license renew [--system]               (alias: refresh)
```

- **`--system`** acts on the machine-wide licence store, for every user of
  the machine, instead of the current user's. It needs administrator
  (Windows) or root (macOS/Linux) rights. Installers use it. See
  [Where licences live](#where-licences-live).
- **`--license-file <PATH>`** is a global flag, accepted on any command
  including a deletion run. It uses a licence file in place without
  installing it, the same as `CURSDEL_LICENSE_FILE`, which suits CI and
  automation.

### `status`

Shows the edition in effect, the licence behind it (id, term,
organisation, signing key and algorithm, source file), the Business
activation if any, which gated features are enabled, and every licence
file that was found but is **not** in use, with the reason.

```console
$ cursdel license status
CurseDelete License Status

Edition:      Community (unregistered)
Product:      cursedelete v2

No licence is in effect -- running with Community capabilities.
Install a licence file with 'cursdel license install <file>', or
activate online with 'cursdel license activate <key>'.

Features:
  --close-remote-locks  not included in this edition
```

`--json` prints the same information as a JSON object (`edition`,
`registered`, `product`, `license`, `activation`, `features`,
`rejected`) for scripts and fleet tooling.

### `install <PATH>`: offline

Validates a signed licence file and copies it into the licence store
(per-user, or machine-wide with `--system`). No network access, no
device information. This is how
Community registrations, Education and Enterprise licences, and
portal-downloaded Business renewals are normally installed. A file that
fails validation is rejected with the reason and nothing changes, so an
invalid file can never replace a valid one.

Installing a Business licence file does not activate it: Business still
needs `activate` once per device.

### `activate <KEY>`: online

Sends the activation key from your purchase or the customer portal to
the licensing server and stores what comes back, after validating it:

- **Business:** registers this device against one of the licence's seats
  (re-activating the same device re-uses its seat) and stores the signed
  activation certificate plus a locally generated secret used later for
  `renew`/`deactivate`. The only thing identifying the device is a
  SHA-256 hash of the OS machine id. No hostname, hardware details or
  personal data are sent.
- **Community / Education / Enterprise:** downloads the licence. No seat,
  no device record.

If the server is unreachable or refuses, nothing local changes.

### `deactivate`: Business only

Asks the server to release this device's seat. Only after the server
confirms is the local activation certificate removed. The licence file
stays installed, so you can activate again later. If the server can't be
reached, nothing is removed and the command says the seat was **not**
released. If the seat was already released through the customer portal,
the local activation is cleaned up.

Releasing a seat counts towards the licence's rolling transfer allowance,
which the server enforces.

### `renew`

Downloads the current signed licence for the installed licence, for
example after a subscription renewal, and for Business also a refreshed
activation certificate. It works on an expired subscription too, since
that is when renewal matters. The new licence is validated before
anything is replaced, and a failed or unreachable renewal leaves the
existing licence exactly as it was.

`renew` is also how server-side changes reach a machine:

- If the licence has been **revoked**, CurseDelete records that locally
  and stops using it. The file is kept, so a later successful renewal,
  for example after support reverses the revocation, restores it.
- If this device's Business activation was **released** (through the
  customer portal or by support), the local activation is removed and
  Business features stop until you `activate` again.

## Offline use and revocation

- **Community, Education, Enterprise:** fully offline, indefinitely.
  Install the file and never contact the server again.
- **Business:** contacts the server once to activate, then works offline
  indefinitely (a perpetual licence) or until its subscription
  `expiresAt` (a subscription). When a subscription expires, the gated
  features turn off and CurseDelete keeps working as Community; nothing
  else is lost. `renew` restores them.
- **Revocation and portal deactivation** take effect on the server
  immediately: the seat is free and future activation and renewal
  requests are refused. A copy already installed on a machine that stays
  offline cannot be switched off remotely. CurseDelete deliberately
  allows indefinite offline use, so the change reaches that machine only
  at its next `renew` (see above).
- **Administrator-provided revocation:** list licence ids in the
  machine-wide store's `revocations.json` as
  `{"revokedLicenseIds": ["lic_..."]}` and CurseDelete will not use those
  licences, for example in an air-gapped estate. The list is unsigned on
  purpose: it can only take entitlement away.

## Where licences live

Everything is per major version (`v2`), so different major versions
installed side by side keep separate licences. There are two stores:

| Platform | User store (default) | Machine-wide store (`--system`) |
|---|---|---|
| Windows | `%LOCALAPPDATA%\RePassCloud\CurseDelete\v2\` | `%ProgramData%\RePassCloud\CurseDelete\v2\` |
| macOS | `~/Library/Application Support/CurseDelete/v2/` | `/Library/Application Support/CurseDelete/v2/` |
| Linux | `~/.config/cursedelete/v2/` (honours `XDG_CONFIG_HOME`) | `/etc/cursedelete/v2/` |

`CURSDEL_LICENSE_DIR` replaces the machine-wide directory. Licences are
considered in this order:

1. `--license-file` / `CURSDEL_LICENSE_FILE`: one explicit file, never
   modified.
2. The user store's `license.json`.
3. Every `*.license` or `*.json` licence file in the machine-wide store:
   the one `--system` commands write, plus any an administrator copies
   in.

If several are valid, the highest edition wins. `license status` lists
the rest and why they are not in use.

Each store can also hold a Business activation (`activation.json`, the
signed certificate, and `credentials.json`, the activation id and its
client secret) and `revocations.json`. The client secret is a bearer
credential and is never printed. It is owner-only (`0600`) on Unix; in
the machine-wide store on Windows it is restricted to SYSTEM and
Administrators, so `deactivate --system` and `renew --system` need an
elevated prompt. Files are written atomically, so an interrupted write
can't corrupt a licence. They are protected files rather than OS
keychain entries because keychain prompts would break unattended and CI
use.

## Deploying to many machines

- **Education / Enterprise / registered Community:** put the licence file
  in the machine-wide store, by `cursdel license install --system <file>`,
  configuration management copying it into the directory above, or the
  Windows installer parameters below. Nothing contacts the server and no
  device information is collected.
- **Business:** each machine activates once:
  `cursdel license activate --system <KEY>`, or the installer's
  activation-key parameter. Each machine uses one seat.
- **Windows installers:** Inno Setup `/LICENSEFILE="<path>"` or
  `/ACTIVATIONKEY="<key>"`; MSI `LICENSEFILE="<absolute path>"` or
  `ACTIVATIONKEY="<key>"`. A licensing problem never fails the install.
  Check afterwards with `cursdel license status`. See
  [build/windows/README.md](../build/windows/README.md#licensing-at-install-time).
- **CI / ephemeral runners:** pass `--license-file` or set
  `CURSDEL_LICENSE_FILE`. Nothing needs to be installed.

## How verification works

Every run that needs the edition verifies the licence locally, with no
network:

1. The envelope's signatures are checked against the public keys built
   into this release. Each key uses exactly one algorithm: Ed25519
   (default), ECDSA-P256-SHA256, or ML-DSA-65 (post-quantum). A licence
   may carry more than one signature, so builds trusting different keys
   can accept the same file.
2. Then the signed claims are checked: product `cursedelete`, major
   version `2`, edition, and expiry.
3. For Business, the signed activation certificate must also match this
   licence and this device.

Any change to a signed claim invalidates the signature. The file format
and the server API are specified in
[LICENSING-SERVER-BRIEF.md](LICENSING-SERVER-BRIEF.md).

> **Note:** the licensing server publishes the public keys a release
> trusts, and they are compiled into that release. Until the first keys
> are added, every licence is reported as signed by an untrusted key and
> CurseDelete runs as Community.

ECDSA-P256-SHA256 and ML-DSA-65 are FIPS-approved algorithms, but
CurseDelete's pure-Rust implementations are not FIPS-validated modules.
Being able to verify those algorithms does not make CurseDelete
FIPS-compliant.

## `CURSDEL_LICENSE_SERVER_URL`

The licensing server's base URL, used only by `activate`, `deactivate`
and `renew`. It must be `https://` (plain `http://` is accepted only for
`localhost`/`127.0.0.1`/`[::1]` development). Release builds can have a
default compiled in (the release workflow reads the
`CURSDEL_LICENSE_SERVER_URL` repository variable). If neither is set,
those three commands say no server is configured and everything offline
keeps working.
