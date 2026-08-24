# Changelog

All notable changes to CurseDelete 2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(the workspace version in `Cargo.toml`'s `[workspace.package]` is the single
source of truth; see `docs/RELEASE.md` for how a release is cut).

## [Unreleased]

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
  machine reports success without contacting the server or consuming
  another seat. See `docs/LICENSING.md#unattended-enrollment-with-a-deployment-key`.
- Machine-wide licence/activation storage (`cursdel_license::store::machine_wide_paths`),
  written by `license enroll`: `%PROGRAMDATA%\RePassCloud\CurseDelete\` on
  Windows, `/var/lib/repasscloud/cursedelete/` on Linux, and
  `/Library/Application Support/RePassCloud/CurseDelete/` on macOS. Manual
  `activate`/`import` continue to use the existing per-user location;
  `license status`, `license refresh`, and `license deactivate` now check
  machine-wide storage first, then fall back to per-user storage. Requires
  administrator/root privileges to write; enrollment fails clearly rather
  than silently falling back to per-user activation if the process lacks
  them.

### Changed

- The license server base URL (`CURSDEL_LICENSE_SERVER_URL` still
  overrides it) now defaults to the RePass Cloud-operated production
  server, `https://license-server.repasscloud.com`, instead of the local
  development URL — production use needs no configuration.

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
