# Command Reference

Complete reference for every `cursdel` flag and subcommand, organized by
concern. This is the exhaustive reference; for a guided first run see
[QUICKSTART.md](QUICKSTART.md). Flags are pulled directly from
[`crates/cursdel-cli/src/args.rs`](../crates/cursdel-cli/src/args.rs) — if a
flag isn't listed here, it doesn't exist; run `cursdel --help` at any time to
confirm against your installed build.

```text
cursdel <TARGET> [OPTIONS]
cursdel license <SUBCOMMAND>
```

`TARGET` is required unless a subcommand (currently only `license`) is
given instead.

## Targeting

### `<TARGET>` (positional, required)

The file or directory to delete. Must exist, and must not be — or resolve
to, via `..` collapse or a symlink/junction — a filesystem root or SMB/UNC
share root. See [SAFETY_MODEL.md](SAFETY_MODEL.md) for the exact rules and
examples of what is rejected and why. A single file target deletes just
that file; a single symlink target deletes the link itself, never whatever
it points to.

## Mode

These three flags are mutually exclusive in effect (increasing order of
aggressiveness); passing more than one, `--destroy` wins.

### `--force`

Attempts permission/attribute/ownership/ACL remediation when a plain
deletion attempt fails, but only where the executing security context
already has the authority to make that change. `--force` never bypasses the
OS security model — it clears blocking attributes, adjusts permissions, or
takes ownership only when the account CurseDelete runs as is allowed to.
Without `--force`, a permission failure is reported and left alone; the
normal, unremediated path stays fast because expensive ACL/ownership
inspection is skipped entirely unless a plain delete already failed.

### `--destroy`

`--force`, plus adaptive maximum-throughput operation and local lock
termination (equivalent to `--force --kill-locks`, plus always running with
`--workers auto`). `--destroy` does **not** imply `--close-remote-locks` —
administratively closing a file open on someone else's server session is a
materially different, higher-risk action than local remediation, so it
always requires its own explicit flag regardless of mode.

### (no mode flag / "normal")

Plain deletion: no remediation is attempted. This is the default and the
fastest path — appropriate when you already know the target is deletable
without any permission surgery.

## Workers

### `--workers <auto|N>`

Default: `auto`.

- `auto` starts from a small CPU-scaled seed (clamped between 4 and 16) and
  lets an adaptive hill-climbing controller grow or shrink concurrency
  every ~400ms based on observed throughput and error rate, up to a
  CPU-scaled ceiling (16 workers per logical CPU, clamped between 32 and
  256 — e.g. 160 on a 10-core machine). The ceiling is proportional to CPU
  count, not a flat constant, because every worker up to it is pre-spawned
  as a real OS thread regardless of whether the adaptive controller ever
  grows into it (see [ADR-0003](adr/0003-adaptive-workers.md)); a flat
  256-thread pre-spawn measurably hurt small-tree performance in
  benchmarking (see [docs/BENCHMARKS.md](BENCHMARKS.md)) purely from
  fixed thread-creation overhead, which the CPU-scaled ceiling fixes. This
  is the right choice for almost every invocation — the optimal
  concurrency for a local NVMe drive, a spinning disk, and a high-latency
  SMB share are wildly different, and `auto` finds a reasonable point on
  each without you having to know which one you're facing. See
  [ADR-0003](adr/0003-adaptive-workers.md) for exactly how the controller
  decides, including why it backs off immediately once errors exceed 15%
  of a sampling window.
- `N` (a positive integer) disables the adaptive controller entirely and
  uses exactly `N` worker threads for the whole run — deterministic,
  reproducible, and appropriate for benchmarking or for a target you've
  already tuned by hand (e.g. a known-good `--workers 64` for a specific
  file server). `--workers 0` is rejected.

Worker count governs file deletion only; directory removal always runs on
its own small, fixed pool (4 threads) so a large file-delete workload can
never starve directory cleanup and vice versa — see
[ADR-0002](adr/0002-streaming-pipeline.md).

## Retention (age-based cleanup)

See [RETENTION.md](RETENTION.md) for full semantics, including the
directory root-preservation rule and per-platform timestamp availability.

### `--age <DURATION>`

Delete only files at least this old. A unit is **mandatory** — `--age 2` is
rejected outright, because "2" is ambiguous between two minutes and two
weeks and getting that wrong on a destructive tool is a real risk, not a
minor inconvenience. Accepted units (case-insensitive, several spellings
each): `m`/`min`/`mins`/`minute`/`minutes`, `h`/`hr`/`hrs`/`hour`/`hours`,
`d`/`day`/`days`, `w`/`wk`/`wks`/`week`/`weeks`. Fractional values are
accepted (`1.5h`). Examples: `--age 2d`, `--age 90d`, `--age 12h`,
`--age 4w`.

### `--age-by <modified|created|accessed>`

Default: `modified`. Selects which timestamp `--age` compares against.
`created` and `accessed` availability is platform- and filesystem-dependent
— see [RETENTION.md](RETENTION.md#per-platform-timestamp-availability) for
what each supported platform actually reports today. A file whose selected
timestamp is unavailable is retained (never deleted on an unverifiable
basis), not treated as infinitely old.

## Filters

See [FILTERS.md](FILTERS.md) for glob syntax and the exclude-wins
precedence rule in detail.

### `--include <GLOB>`

Only delete files whose name matches this glob pattern (matched against the
file's base name, not the full path). Directories are never matched
directly by `--include`/`--exclude` — see [FILTERS.md](FILTERS.md).

### `--exclude <GLOB>`

Never delete files whose name matches this glob pattern. Always wins over
`--include` when both match the same file.

### `--min-size <SIZE>`

Only delete files at least this size. Bare numbers mean bytes; suffixes are
binary (1024-based): `k`/`m`/`g`/`t` (also accepts `kb`/`mb`/`gb`/`tb`,
`kib`/`mib`/`gib`/`tib` — all treated identically). See
[FILTERS.md](FILTERS.md#size-unit-convention) for why this deliberately
does not follow SI/decimal conventions.

### `--max-size <SIZE>`

Only delete files at most this size. Same unit syntax as `--min-size`.

## Locks

See [LOCKS.md](LOCKS.md) for per-platform mechanism details and honest
capability limits.

### `--kill-locks`

On a sharing/busy failure, attempt to identify the local process holding
the file open and terminate it, then retry the delete. Never terminates
CurseDelete's own process or a protected system process (PID 1 on POSIX).
Implied by `--destroy`.

### `--close-remote-locks`

Windows-server-specific: administratively close a matching remote SMB open
on a supported Windows file server, then retry. Requires both suitable
administrative rights on that file server and a Business or Enterprise
license (the one capability the product explicitly reserves — see
[LICENSING.md](LICENSING.md)). Distinct from `--kill-locks` and **never**
implied by `--destroy`, because closing another user's server-side file
handle is a materially higher-risk action than local remediation and always
requires its own explicit opt-in. On any platform/server combination where
this isn't supported, CurseDelete reports the failure honestly rather than
pretending to have handled it.

## Dry run and output

### `--dry-run`

Plan and report the operation without deleting or modifying anything on
disk. Combine with retention/filters to preview exactly what a policy
change would do before running it for real. See the text-output shape
difference (a "Would delete" / "Would retain" breakdown) in
[QUICKSTART.md](QUICKSTART.md) and [JSON_OUTPUT.md](JSON_OUTPUT.md).

### `--json`

Emit a single machine-readable JSON report to stdout instead of the text
summary. See [JSON_OUTPUT.md](JSON_OUTPUT.md) for the full schema. Not
affected by `--quiet`.

### `--quiet`

Suppress the normal text summary; only failure lines are printed (to
stderr). Does not affect `--json` output, which is always printed in full
when requested.

### `--verbose`

Increase diagnostic detail. Per-file verbose logging measurably reduces
throughput on large trees — it exists for diagnostics, not for routine
high-volume operation.

### `--log <PATH>`

Also write the rendered report (text or JSON, matching whichever was
selected) to this file, in addition to stdout/stderr.

## Global

### `-h`, `--help`

Print help for the current command or subcommand.

### `-V`, `--version`

Print the bare `cursdel <semver>` version and exit — deliberately stable
and scriptable (`cursdel -V | cut -d' ' -f2`), never gains extra lines.

### `--version` (long form)

The long `--version` flag prints an additional line with build profile and
the running binary's actual OS/architecture, useful when reporting a
problem:

```console
$ cursdel --version
cursdel 2.0.0
build:    release (macos-aarch64)
```

`-V` and `--version` are otherwise the same flag; only the long spelling
gets the extra line.

## `license` subcommand

Manages license activation. See [LICENSING.md](LICENSING.md) for the full
model (editions, what's technically gated vs. a licensing-terms
distinction, storage locations, and the `CURSDEL_LICENSE_SERVER_URL`
environment variable).

```text
cursdel license status
cursdel license activate --license-id <ID> --activation-code <CODE> [--offline] [--output <PATH>]
cursdel license import <PATH>
cursdel license deactivate
cursdel license refresh
```

| Subcommand | Purpose |
|---|---|
| `status` | Show the current license/activation status, or confirm the device is running on Community capabilities. |
| `activate --license-id <ID> --activation-code <CODE>` | Online activation: contacts the license server directly and persists the returned signed license. |
| `activate --license-id <ID> --activation-code <CODE> --offline [--output <PATH>]` | Air-gapped activation: writes a request file (default `./offline-activation-request.json`) to email to support instead of contacting the server directly. |
| `import <PATH>` | Import a signed license file received back from an offline activation request. |
| `deactivate` | Free this device's activation so the license can be activated on another device. |
| `refresh` | Renew the current online activation lease before it expires. No-op/error for offline-mode activations, which have no lease to refresh. |

## Verified `--help` output

Captured directly from the built binary (`cursdel --help`), reproduced here
so this reference and the tool itself can't silently drift apart — if they
ever disagree, trust the live `--help` output and treat this document as
needing an update.

```text
CurseDelete: a native, high-performance deletion engine.

Usage: cursdel [OPTIONS] [TARGET] [COMMAND]

Commands:
  license  Manage CurseDelete's licence activation
  help     Print this message or the help of the given subcommand(s)

Arguments:
  [TARGET]  Path to delete (file or directory). Required unless a subcommand (e.g. `license`) is
            given instead

Options:
      --force
          Attempt permission/attribute/ownership/ACL remediation on failure, where the executing
          security context has authority to do so
      --destroy
          Adaptive maximum-throughput deletion with attribute/ownership/ACL remediation and local
          lock termination. Does not imply --close-remote-locks
      --workers <auto|N>
          'auto' (default) or a positive integer worker count. A manual count disables adaptive
          tuning entirely [default: auto]
      --age <DURATION>
          Delete only files at least this old. Requires a unit: m/h/d/w (e.g. 2d, 90d, 12h). A bare
          number is rejected
      --age-by <modified|created|accessed>
          Which timestamp --age compares against [default: modified]
      --include <GLOB>
          Only delete files whose name matches this glob pattern
      --exclude <GLOB>
          Never delete files whose name matches this glob pattern (wins over --include)
      --min-size <SIZE>
          Only delete files at least this size (e.g. 100m, 1g)
      --max-size <SIZE>
          Only delete files at most this size
      --kill-locks
          Identify and terminate local processes holding a blocking file handle, then retry. Never
          touches CurseDelete itself or critical system processes
      --close-remote-locks
          Windows only: administratively close a matching remote SMB open on a supported Windows
          file server, then retry. Requires suitable administrative rights on that server and a
          Business/Enterprise licence. Distinct from --kill-locks and never implied by --destroy
      --dry-run
          Plan the operation without deleting or modifying anything
      --json
          Emit a machine-readable JSON report instead of text
      --quiet
          Suppress the normal summary; only failures/errors are printed
      --verbose
          Increase diagnostic detail. Verbose per-file logging materially reduces throughput on
          large trees and should be used for diagnostics, not routine operation
      --log <PATH>
          Also write the report to this file (in addition to stdout/stderr)
  -h, --help
          Print help
  -V, --version
          Print version
```
