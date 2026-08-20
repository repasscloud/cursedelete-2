# Changelog

All notable changes to CurseDelete 2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(the workspace version in `Cargo.toml`'s `[workspace.package]` is the single
source of truth; see `docs/RELEASE.md` for how a release is cut).

## [Unreleased]

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
