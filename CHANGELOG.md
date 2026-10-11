# Changelog

All notable changes to CurseDelete 2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(the workspace version in `Cargo.toml`'s `[workspace.package]` is the single
source of truth; see `docs/RELEASE.md` for how a release is cut).

## [Unreleased]

Targets 2.1.0 (the workspace version is already bumped).

### Changed

- **Licensing rewritten** to the cross-product architecture in
  `docs/LICENSING-ARCHITECTURE.md` (see ADR-0010). The 2.0.0 licensing
  (`software-license-v1`, test licences only) is not supported.
  - `cursdel-license` is now product-agnostic, and `cursdel-policy` holds
    CurseDelete's licensing configuration and edition → feature table.
  - New signed envelope with detached signatures: Ed25519 (default),
    ECDSA-P256-SHA256, and ML-DSA-65. Each key is bound to one algorithm,
    each application has an allowlist, and licences can be dual-signed.
  - Licences are per product and major version. Business activation is a
    separate signed certificate, and activation is once per device
    followed by indefinite offline use.
  - `cursdel license` is now `status [--json]`, `install` (alias
    `import`), `activate <KEY>`, `deactivate`, and `renew` (alias
    `refresh`). `--system` selects the machine-wide licence store, and a
    global `--license-file` uses a licence file without installing it.
    `activate --license-id/--activation-code/--offline` were removed.
  - Licences are read from `--license-file`/`CURSDEL_LICENSE_FILE`, the
    per-user store, and the machine-wide store (`CURSDEL_LICENSE_DIR` or
    the platform default), all under `…/v2/`.
  - `renew` applies a revocation or released Business activation that the
    server reports. Administrators can also list revoked licence ids in
    the machine-wide `revocations.json`.
  - The licensing server URL must be `https://`; release builds can bake
    one in from the `CURSDEL_LICENSE_SERVER_URL` repository variable.
- **Windows installers:** `/LICENSEFILE=` / `/ACTIVATIONKEY=` (Inno) and
  `LICENSEFILE` / `ACTIVATIONKEY` (MSI) now license the machine-wide
  store. They replace the `license enroll --deploymentkey` call, a
  command that never existed in the Rust CLI, and the step that granted
  Users write access to JSON files in Program Files.
- Minimum supported Rust version raised to 1.85 (required by `ml-dsa`).

### Added

- `docs/LICENSING-SERVER-BRIEF.md`: the specification for the
  replacement licensing server, with test vectors for all three
  algorithms that are verified in CI. Also the `cursdel-license`
  `conformance` example, which validates server output with the client's
  own code.

### Security

- `rustls` 0.23.43 → 0.23.45 (RUSTSEC-2026-0285).

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

[Unreleased]: https://github.com/danijeljw-RPC/cursedelete-2/compare/v2.0.0...dev
[2.0.0]: https://github.com/danijeljw-RPC/cursedelete-2/releases/tag/v2.0.0
