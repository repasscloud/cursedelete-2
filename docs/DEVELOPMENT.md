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
├── cursdel-linux/      native Linux deletion engine (stub — see
│                       "Platform implementation status" below).
└── cursdel-windows/    native Windows deletion engine (stub — see
                        "Platform implementation status" below).
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
| Linux | Not yet implemented (`crates/cursdel-linux/src/lib.rs` is a stub — `// Implemented in a later step.`). |
| Windows | Not yet implemented (`crates/cursdel-windows/src/lib.rs` is a stub, same as above). |

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

This runs all unit tests across every crate (140 passing as of this
writing: 74 in `cursdel-core`, 44 in `cursdel-license`, 18 in
`cursdel-macos`, 4 in `cursdel-policy`; `cursdel-cli`,
`cursdel-linux`, and `cursdel-windows` currently have none of their own).
The macOS engine's own test suite includes an end-to-end run of the full
`cursdel-core` pipeline against the real engine over a temporary directory
tree — the closest thing to an integration test currently in the
repository.

There is no separate `tests/integration/`, `tests/destructive/`, or
`tests/benchmark/` directory at the workspace root as of this writing,
despite both being referenced in the original architecture brief
(`README-CurseDelete2.md`) and `docs/adr/0003-adaptive-workers.md`
referencing a `docs/BENCHMARKS.md` that also does not exist yet. Treat
these as planned, not shipped — the license crate does ship real,
non-synthetic fixtures under `crates/cursdel-license/tests/fixtures/` (see
[ADR-0004](adr/0004-licensing-integration.md)), which is the one place a
`tests/` directory currently exists in the workspace.

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
