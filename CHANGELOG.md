# Changelog

All notable changes to CurseDelete 2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(the workspace version in `Cargo.toml`'s `[workspace.package]` is the single
source of truth; see `docs/RELEASE.md` for how a release is cut).

## [Unreleased]

## [2.1.0] - 2026-08-25

### Added

- `cursdel license force-deactivate` (issue #13): recovers a machine
  stranded with an active seat on the licence server but no matching local
  `activation.json` — the scenario the write-access preflight check below
  exists to prevent going forward, but which could still occur from a
  version predating that check, or from local credentials lost after
  enrollment otherwise completed. Neither `license status` nor `license
  deactivate` can help in that state, since both only ever look at local
  files, and a retried `license enroll` would hit a `409 Conflict` against
  a seat it can no longer identify itself with. `force-deactivate`
  authenticates with the Deployment Key itself instead of the missing
  `activation_token`, via the licence server's
  `POST /api/v1/deployment-keys/force-deactivate` endpoint (rate-limited
  more strictly, and audited, server-side); it accepts the key via the
  same `--deployment-key`/`--deployment-key-env`/`--deployment-key-file`/
  `--deployment-key-stdin` flags as `enroll`, and clears any stale local
  machine-wide state on success. See
  `docs/LICENSING.md#recovering-a-stranded-machine-with-license-force-deactivate`.

### Fixed

- `cursdel license activate` (issue #13): now verifies it can persist
  activation state locally *before* contacting the licence server, the
  same preflight `license enroll` already ran for machine-wide state.
  Previously, `activate` requested and completed activation with the
  server first and only then tried to save the result — a local write
  failure after that point (missing directory, read-only filesystem,
  permissions) left the device holding an active seat on the server with
  no local credentials to show for it, and no CLI path to recover (a
  retried `activate` would be rejected, since the server still considers
  the device already active, and releasing the seat normally needs the
  `activation_token` that was never actually saved). See
  `docs/LICENSING.md#write-access-preflight`.

## [2.0.1] - 2026-08-25

### Added

- `cursdel license enroll`: unattended, Deployment Key-based machine
  enrollment (issue #9), for provisioning tooling (Intune, an RMM, a
  golden VM image, a container entrypoint) that can't type an interactive
  activation code. A Deployment Key is an enrollment credential, not a
  licence — it is used once to obtain a normal machine-bound activation
  and is never itself persisted, logged, or printed. Accepts the key via
  `--deployment-key`, `--deployment-key-env`, `--deployment-key-file`, or
  `--deployment-key-stdin` (exactly one required); the non-CLI-argument
  forms avoid putting the secret on a process command line for automated
  environments. Re-running `enroll` on an already-enrolled, still-valid
  machine (a matched, verified license *and* its activation credentials)
  reports success without contacting the server or consuming another seat;
  a partial prior enrollment (license saved but its credentials weren't)
  is treated as incomplete and retried, not as already-enrolled. See
  `docs/LICENSING.md#unattended-enrollment-with-a-deployment-key`.
- Machine-wide licence/activation storage and machine → user → Community
  resolution (issue #8): `license status` and ordinary deletion now check
  machine-wide storage before per-user storage, and `license refresh`/
  `deactivate` resolve to whichever scope actually holds activation
  credentials. An invalid machine-scope license is reported, not silently
  bypassed in favour of a valid user-scope one — see
  `cursdel_license::resolve_active_license` and
  `docs/adr/0004-licensing-integration.md`. `license enroll` requires
  administrator/root privileges to establish machine-wide state and fails
  clearly, before any network call, if it can't — never a silent
  downgrade to per-user activation.

### Changed

- The license server base URL (`CURSDEL_LICENSE_SERVER_URL` still
  overrides it) now defaults to the RePass Cloud-operated production
  server, `https://license-server.repasscloud.com`, instead of the local
  development URL — production use needs no configuration.
- License/activation storage now lives under `CurseDelete-2`/`cursedelete-2`
  rather than `CurseDelete`/`cursedelete`, so this Rust rewrite's license
  state can never collide with the separate, pre-rewrite C# CurseDelete
  implementation:
  - User scope: `%LOCALAPPDATA%\RePassCloud\CurseDelete-2\` (Windows),
    `~/.config/cursedelete-2/` (Linux), `~/Library/Application Support/CurseDelete-2/` (macOS).
  - Machine scope (new, written by `license enroll`): `%PROGRAMDATA%\CurseDelete-2\`
    (Windows), `/var/lib/cursedelete-2/` (Linux),
    `/Library/Application Support/CurseDelete-2/` (macOS). Machine-scope
    `activation.json` is additionally restricted to Administrators/SYSTEM
    on Windows via an explicit `icacls` grant, since `%ProgramData%` is
    otherwise readable by every local account.
  - A machine that activated under v2.0.0's original (un-suffixed) paths
    needs to re-activate/re-enroll once.
  - The Windows MSI/Inno/MSIX installers under `build/windows/` are
    updated to match: the post-install enrollment step now invokes
    `license enroll --deployment-key=...` (previously a typo'd
    `--deploymentkey=` that Clap would have rejected outright), and the
    installers' own JSON-file ACL step — which targeted the install
    directory, never where these files are actually written — is removed
    in favour of `cursdel` applying the correct permissions itself.

## [2.0.0] - 2026-08-17

### Added

- Initial CurseDelete 2 Rust rewrite: Cargo workspace with shared
  orchestration (`cursdel-core`), platform deletion engines
  (`cursdel-windows`, `cursdel-linux`, `cursdel-macos`), licensing
  (`cursdel-license`, `cursdel-policy`), and the `cursdel` CLI
  (`cursdel-cli`). See `docs/adr/` for the architectural decisions behind
  the crate split and `README.md#appendix-original-productarchitecture-brief` for the product brief this
  rewrite implements.
- CI (`.github/workflows/ci.yml`): formatting, clippy, `cargo test` across
  Windows/Linux/macOS, cross-target type-checking for the ARM64 shipping
  targets, and dependency/license auditing via `cargo-deny`.
- Release packaging (`.github/workflows/release.yml`): tag-triggered build
  matrix producing archives for all six shipping targets (Windows
  x64/ARM64, Linux x64/ARM64, macOS Apple Silicon/Intel), published to
  GitHub Releases with checksums. See `docs/RELEASE.md` for the process and
  what's still manual (code signing, WinGet, Linux package managers).

[Unreleased]: https://github.com/danijeljw-RPC/cursedelete-2/compare/v2.0.1...dev
[2.0.1]: https://github.com/danijeljw-RPC/cursedelete-2/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/danijeljw-RPC/cursedelete-2/releases/tag/v2.0.0
