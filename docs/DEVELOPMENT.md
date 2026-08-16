# Development

Building, testing, and working across the workspace. For the product/user
docs, start at [README.md](../README.md); for the architecture decisions
behind the crate layout, see the [ADR index](adr/README.md).

## Requirements

- Rust, edition 2021, `rust-version = "1.82"` (see the workspace
  `Cargo.toml`). No `rust-toolchain.toml` is pinned in the repository as of
  this writing — use a current stable toolchain via `rustup`.
- No other runtime dependency (no .NET SDK, no external services) is
  required to build or test the parts of the workspace that build on your
  host OS — see [ADR-0004](adr/0004-licensing-integration.md) for why the
  license verifier deliberately has no .NET dependency despite
  interoperating with a .NET-based license server.

## Building

```bash
cargo build --workspace
```

or, for just the CLI binary:

```bash
cargo build -p cursdel-cli
./target/debug/cursdel --help
```

A release build:

```bash
cargo build --workspace --release
```

## Crate layout

```text
crates/
├── cursdel-core/      shared orchestration: pipeline, adaptive workers,
│                       target safety, filters, retention, reporting.
│                       No platform-specific code.
├── cursdel-cli/        the `cursdel` binary: argument parsing, wiring to
│                       cursdel-core::pipeline, platform engine selection.
├── cursdel-policy/     maps a verified license entitlement to a resolved
│                       capability set. Never consumed by cursdel-core.
├── cursdel-license/    offline license verification + online activation
│                       client. No dependency on cursdel-core.
├── cursdel-macos/      native macOS deletion engine (implemented).
├── cursdel-windows/    native Windows deletion engine (implemented).
└── cursdel-linux/      native Linux deletion engine (implemented).
```

Each platform crate implements exactly one trait,
[`cursdel_core::engine::PlatformEngine`](../crates/cursdel-core/src/engine.rs),
and is `cfg`-gated at the crate root (`#![cfg(target_os = "macos")]`,
etc.) so it compiles to an empty crate on other hosts rather than being
excluded from the workspace graph — this keeps `cargo check --workspace`
meaningful on every development machine. The genuinely
platform-independent parts of tree traversal (explicit-stack walking so
arbitrarily deep trees can't blow the call stack, directory-ID allocation,
directory-completion events) are implemented exactly once, in
`cursdel_core::walk::stream_tree`; a platform engine only needs to supply a
`DirLister` that lists one directory using its fastest native call. See
[ADR-0001](adr/0001-workspace-architecture.md) for the full reasoning,
including why this crate split was chosen over both prior CurseDelete
implementations' approaches.

## Platform implementation status

| Platform | Status |
|---|---|
| macOS | Implemented, tested. Reference implementation for the `PlatformEngine` trait. |
| Windows | Implemented (`FindFirstFileExW` enumeration, `FILE_DISPOSITION_INFO_EX` deletion with a classic fallback, ownership/ACL remediation, Restart Manager for `--kill-locks`, `NetFileEnum`/`NetFileClose` for `--close-remote-locks` — see [ADR-0007](adr/0007-windows-engine.md)). Validated by cross-compilation (`cargo check`/`clippy` for `x86_64-pc-windows-msvc` and `x86_64-pc-windows-gnu`) and unit tests for all logic that doesn't require a live Windows session; real-filesystem behaviour is marked with `TODO(windows-ci)` comments pending validation on an actual Windows machine. |
| Linux | Implemented (`openat`/`unlinkat` deletion with the same TOCTOU-mitigation pattern as macOS, `statx` metadata with an `fstatat` fallback for kernels/filesystems without `STATX_BTIME`, native `/proc/[pid]/fd`-based lock-holder discovery for `--kill-locks` — see [ADR-0008](adr/0008-linux-engine.md)). Validated by cross-compilation (`cargo check`/`clippy` for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`) and unit tests for all logic that doesn't require a live Linux session; real-filesystem behaviour is marked with `TODO(linux-ci)` comments pending validation on actual Linux hardware. |

`cursdel-core` (the streaming pipeline, adaptive worker controller, target
safety, filters, retention, and reporting) is fully platform-independent
and fully tested on any host, including macOS, without needing real
Windows or Linux hardware — see [ADR-0001](adr/0001-workspace-architecture.md)
and [ADR-0003](adr/0003-adaptive-workers.md) for how the adaptive
controller specifically is validated against synthetic throughput curves
rather than requiring real NVMe/HDD/SMB hardware in CI.

## Running tests

```bash
cargo test --workspace
```

This runs all unit and integration tests across every crate (179 passing
as of this writing: 76 in `cursdel-core`, 49 in `cursdel-license`, 32 CLI
integration tests in `cursdel-cli`, 18 in `cursdel-macos`, 4 in
`cursdel-policy`; `cursdel-linux` and `cursdel-windows` currently run 0 of
their own on a macOS host since both are `cfg`-gated to their target OS —
their unit tests exist in source and run in CI on their native platform).
The macOS and Windows engines' own test suites each include an end-to-end
run of the full `cursdel-core` pipeline against the real engine over a
temporary directory tree; `cursdel-cli`'s integration suite
(`crates/cursdel-cli/tests/cli_integration.rs`) goes a step further and
spawns the actual `cursdel` binary as a subprocess, including a real
SIGINT sent mid-operation to exercise the documented `Interrupted` exit
code.

`tests/benchmark/` (a Python harness comparing `cursdel` against `rm -rf`
on synthetic trees, see [docs/BENCHMARKS.md](../docs/BENCHMARKS.md) for
real -- not fabricated -- results captured with it) exists at the
workspace root. `tests/integration/` and `tests/destructive/`, referenced
in the original architecture brief (`README-CurseDelete2.md`) as
suggested locations for further destructive/integration coverage, remain
empty placeholders beyond what `crates/cursdel-cli/tests/` and
`crates/cursdel-license/tests/fixtures/` already provide -- the CLI
integration suite already exercises real destructive operations inside
isolated `tempfile::tempdir()` roots, which is where the requirement
("destructive tests must run in isolated temporary test roots") is
actually satisfied today.

### Running a single crate's tests

```bash
cargo test -p cursdel-core
cargo test -p cursdel-macos
```

## Lint and format

```bash
cargo fmt --check
cargo clippy --workspace --all-targets
```

Both are clean on the current codebase. No `rustfmt.toml` or `clippy.toml`
is checked in, so both tools run with their default configuration.

## Cross-target checking

You cannot fully build or run a platform engine for an OS you're not on —
`cargo check`, not `cargo build`/`cargo test`, is what's available for
verifying a non-native platform crate compiles and its API usage
type-checks. Install the targets you need via `rustup`:

```bash
rustup target add x86_64-pc-windows-msvc
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
```

`cursdel-core` (and any crate that only depends on it) has no native/C
dependencies, so checking it against another target works out of the box
on any host:

```bash
cargo check -p cursdel-core --target x86_64-unknown-linux-gnu
```

Checking the **whole workspace** against a foreign target additionally
needs a real cross C toolchain (a linker plus a C compiler for that
target), because `ureq`'s TLS support pulls in `ring`, which compiles C
source as part of its build. On a plain macOS development machine without
Windows/Linux cross-compilation toolchains installed, `cargo check
--workspace --target x86_64-pc-windows-msvc` (or the Linux equivalent)
will fail at the `ring` build step with a missing-compiler or
missing-header error — this is expected on a machine set up only for
native macOS development, not a sign of a broken workspace. CI (once
configured — see below) or a machine with the appropriate cross toolchain
installed can run the full cross-target check; locally, scope `cargo
check` to the platform-independent crates you actually changed.

## Continuous integration

There is no `.github/workflows/` directory in the repository as of this
writing, despite [ADR-0001](adr/0001-workspace-architecture.md) describing
CI running the real test suite on Windows, Linux, and macOS runners. Until
that's added, treat `cargo test --workspace`, `cargo fmt --check`, and
`cargo clippy --workspace --all-targets` (all shown above) as the manual
equivalent to run before proposing a change.

## Licensing integration notes for contributors

If you're changing anything under `crates/cursdel-license/`, read
[ADR-0004](adr/0004-licensing-integration.md) first: the signature
verification path must stay byte-exact with the .NET reference
implementation's canonical JSON serialization, and the real C#-signed test
fixtures under `crates/cursdel-license/tests/fixtures/` are what prove
that, not synthetic Rust-to-Rust round trips. `docs/LICENSING.md` covers
the user-facing behavior; `LICENSING-INTEGRATION.md` at the repository
root is the underlying protocol specification this crate implements.
