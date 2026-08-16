<h1>CurseDelete</h1>

<p><strong>A native, high-performance deletion engine for files and directory trees that refuse to die.</strong></p>

CurseDelete deletes while it enumerates, tunes its own concurrency to the
storage target it's running against, and refuses — structurally, not just
by convention — to ever delete a filesystem or SMB share root. This
repository (`cursedelete-2`) is a from-scratch Rust rewrite of the product;
the CLI binary is `cursdel`.

## Status

The core engine, macOS platform engine, license verification, and CLI are
implemented and tested. Linux and Windows platform engines are stubs under
active development — see [Platform support](#platform-support) below for
exactly what that means today. This is pre-release software; no versioned
release binaries are published yet.

## Install

No prebuilt release binaries are published yet, and there is no CI/release
pipeline (`.github/workflows/`) in the repository as of this writing.
Building from source is currently the only way to get `cursdel`:

```bash
git clone https://github.com/danijeljw-RPC/cursedelete-2.git
cd cursedelete-2
cargo build --release -p cursdel-cli
./target/release/cursdel --version
```

Requires a current stable Rust toolchain (`rust-version = "1.82"`). See
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full build/test/lint
workflow, and [docs/QUICKSTART.md](docs/QUICKSTART.md) for a guided first
run.

## Platform support

| Platform | Engine status |
|---|---|
| macOS | **Implemented and tested.** Native Darwin/POSIX primitives (`openat`/`unlinkat`/`fstatat`), the reference implementation of the `PlatformEngine` trait. |
| Linux | **Not yet implemented.** `crates/cursdel-linux` currently compiles to an empty stub; native `openat`/`unlinkat`/`statx` support via `libc` is planned. |
| Windows | **Not yet implemented.** `crates/cursdel-windows` currently compiles to an empty stub; native Win32 (`FindFirstFileExW`, `FILE_DISPOSITION_INFO_EX`, ACL remediation, Restart Manager) support is planned. |

`cursdel-core` — the streaming pipeline, adaptive worker controller,
target-safety validation, filters/retention, and reporting — is fully
platform-independent and fully tested on any development host today,
including this repository's macOS environment, without needing real
Windows or Linux hardware. See [ADR-0001](docs/adr/0001-workspace-architecture.md)
for how the workspace is structured so that adding a platform engine means
implementing one trait, not re-deriving the whole concurrency/safety
design.

## Quick start

Always preview a destructive command first:

```console
$ cursdel /tmp/scratch/old-build --dry-run
CurseDelete 2

Target:       /tmp/scratch/old-build
Mode:         normal (dry-run)
Workers:      auto -> 10
ACL repair:   disabled
Kill locks:   disabled
Remote locks: disabled

Files scanned:       4
Directories scanned: 3

Would delete:
  Files:             4
  Directories:       3
  Data:              20.0 B

Would retain:
  Files:             0
  Directories:       0

No files were modified.
```

Then run it for real:

```console
$ cursdel /tmp/scratch/old-build
CurseDelete 2

Target:       /tmp/scratch/old-build
Mode:         normal
Workers:      auto -> 10
ACL repair:   disabled
Kill locks:   disabled
Remote locks: disabled

Files:          4
Directories:    3
Deleted:        20.0 B
Failures:       0
Elapsed:        00:00:00
Rate:           19 files/sec

Complete.
```

Age-based retention cleanup is a first-class use case — this deletes only
files at least two days old, and never removes the `Logs` directory itself
even once its old contents are gone:

```bash
cursdel Logs --age 2d --dry-run
```

Both examples above are real, verified output captured from the built
binary. See [docs/QUICKSTART.md](docs/QUICKSTART.md) for a fuller walk
through, including permission remediation and structured JSON output.

## Features

- **Streaming deletion** — enumeration and deletion overlap; deletion
  starts before the tree is fully walked, and memory use stays bounded
  regardless of tree size (backpressure is structural, not bolted on). See
  [ADR-0002](docs/adr/0002-streaming-pipeline.md).
- **Adaptive concurrency** — `--workers auto` (the default) hill-climbs to
  a good worker count for whatever storage target it's actually running
  against, backing off immediately if the error rate spikes; `--workers N`
  gives an exact, deterministic override. See
  [ADR-0003](docs/adr/0003-adaptive-workers.md).
- **Structural root protection** — a filesystem or SMB share root can never
  be deleted, including via `..` collapse or a symlink/junction disguising
  one, enforced through two independent, independently-tested layers. See
  [docs/SAFETY_MODEL.md](docs/SAFETY_MODEL.md).
- **Symlink-safe traversal** — a symlink or junction discovered inside a
  target tree is always deleted as the link object itself, never followed.
  See [ADR-0005](docs/adr/0005-symlink-reparse-safety.md).
- **POSIX TOCTOU mitigation** — deletes are performed via `openat`-relative
  file descriptors rather than re-resolved path strings, closing the
  final-component race. The residual ancestor-chain risk is documented
  honestly, not glossed over. See [ADR-0006](docs/adr/0006-posix-toctou.md).
- **Retention cleanup** — `--age`/`--age-by` with a mandatory duration
  unit, directories removed only once genuinely empty, and the target
  directory itself always preserved under a filtered run. See
  [docs/RETENTION.md](docs/RETENTION.md).
- **Composable filters** — `--include`/`--exclude`/`--min-size`/
  `--max-size`, with exclude always winning ties. See
  [docs/FILTERS.md](docs/FILTERS.md).
- **`--force`/`--destroy` remediation** — permission/ownership/ACL repair
  attempted only on failure, and only within the authority the executing
  account already has — never a security-model bypass.
- **Lock handling** — `--kill-locks` for local process locks (macOS today
  via `lsof`), and a separate, explicitly opt-in, Business/Enterprise-only
  `--close-remote-locks` for supported Windows file servers. See
  [docs/LOCKS.md](docs/LOCKS.md).
- **Stable machine-readable output** — `--json` with a versioned schema,
  and a frozen exit-code contract for automation. See
  [docs/JSON_OUTPUT.md](docs/JSON_OUTPUT.md) and
  [docs/EXIT_CODES.md](docs/EXIT_CODES.md).

## Documentation

| Document | Covers |
|---|---|
| [docs/QUICKSTART.md](docs/QUICKSTART.md) | Install and your first few commands. |
| [docs/COMMAND_REFERENCE.md](docs/COMMAND_REFERENCE.md) | Every flag, with syntax, defaults, and the "why" `--help` doesn't have room for. |
| [docs/SAFETY_MODEL.md](docs/SAFETY_MODEL.md) | Root/share-root protection, symlink safety, TOCTOU mitigation, retention's root-preservation rule. |
| [docs/RETENTION.md](docs/RETENTION.md) | `--age`/`--age-by`, duration syntax, directory cleanup semantics, per-platform timestamp reality. |
| [docs/FILTERS.md](docs/FILTERS.md) | `--include`/`--exclude`/`--min-size`/`--max-size`, glob syntax, precedence. |
| [docs/LOCKS.md](docs/LOCKS.md) | `--kill-locks` and `--close-remote-locks`, per platform. |
| [docs/JSON_OUTPUT.md](docs/JSON_OUTPUT.md) | The `--json` schema, field by field, with a real captured example. |
| [docs/EXIT_CODES.md](docs/EXIT_CODES.md) | The frozen exit code table and what triggers each one. |
| [docs/LICENSING.md](docs/LICENSING.md) | Editions, what's actually technically gated, the `license` subcommand. |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Building, testing, linting, crate layout, cross-target checking. |
| [docs/adr/](docs/adr/README.md) | Architecture decision records — the reasoning behind the design, including where the implementation refines the original brief. |

## Licensing

CurseDelete's core deletion engine — adaptive and manual worker control,
`--force` remediation, retention, filters, dry-run, JSON output — runs at
full, unthrottled speed in every edition, including the free **Community**
edition. Commercial editions (Business, Enterprise) monetize commercial-use
rights, scale, and support, not raw deletion speed; the only capability
actually gated at runtime is `--close-remote-locks`. See
[docs/LICENSING.md](docs/LICENSING.md) for the full edition model,
`cursdel license` usage, and where credentials are stored. See
[LICENSE](LICENSE) for the source code license, and
[LICENSING-INTEGRATION.md](LICENSING-INTEGRATION.md) for the underlying
license-verification protocol this implementation targets.

## About README-CurseDelete2.md

[README-CurseDelete2.md](README-CurseDelete2.md) is the original
product/architecture brief this rewrite was built from. It's kept in the
repository for historical and reference purposes — it describes the
intended design in full, including some forward-looking sections (Windows/
Linux engines, packaging, benchmarking) that are still in progress. Where
the actual implementation refines or deviates from that brief, the
[ADRs](docs/adr/README.md) record why; this README and the rest of `docs/`
describe the product as it actually behaves today.
