# Quickstart

A first run through building CurseDelete and deleting something with it.
For the full flag reference once you're past this, see
[COMMAND_REFERENCE.md](COMMAND_REFERENCE.md).

## Install

No prebuilt release binaries are published yet, and there is no
`.github/workflows/` CI/release pipeline in the repository as of this
writing — building from source is currently the only way to get `cursdel`.
See [README.md](../README.md#install) for current platform build-support
status.

```bash
git clone https://github.com/danijeljw-RPC/cursedelete-2.git
cd cursedelete-2
cargo build --release -p cursdel-cli
```

The binary is at `target/release/cursdel` (`cursdel.exe` on Windows, once
that platform engine ships — see
[README.md](../README.md#platform-support)). Put it on your `PATH`, or
call it by its full path for now:

```bash
./target/release/cursdel --version
```

Requires a current stable Rust toolchain (`rust-version = "1.82"`); see
[DEVELOPMENT.md](DEVELOPMENT.md) for build details and testing.

## Your first delete

Always preview a destructive command with `--dry-run` before running it for
real, especially the first few times:

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

Nothing on disk changed. Once you're satisfied, drop `--dry-run` to run it
for real:

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

A plain `cursdel <path>` with no other flags deletes the target itself —
`old-build` no longer exists after this run. (Output above is a real,
verified capture; your own byte/file counts and timing will naturally
differ.)

## A retention example

Age-based cleanup is a first-class use case. This deletes only files at
least two days old, leaving everything else — and leaving the `Logs`
directory itself in place, even once its old contents are gone (see
[RETENTION.md](RETENTION.md) for exactly why):

```console
$ cursdel Logs --age 2d --dry-run
CurseDelete 2

Target:       Logs
Mode:         normal (dry-run)
Workers:      auto -> 10
ACL repair:   disabled
Kill locks:   disabled
Remote locks: disabled

Files scanned:       4
Directories scanned: 4

Would delete:
  Files:             2
  Directories:       2
  Data:              10.0 B

Would retain:
  Files:             2
  Directories:       1

No files were modified.
```

Drop `--dry-run` once you're confident, and add `--include`/`--exclude`/
`--min-size`/`--max-size` to narrow further — see
[FILTERS.md](FILTERS.md).

## A permission problem

If a plain delete fails because of a file attribute, ownership, or ACL
restriction, add `--force` to have CurseDelete attempt remediation — but
only within the authority the account running it already has; `--force`
never bypasses the OS security model:

```bash
cursdel /tmp/scratch/stubborn --force
```

## Structured output for scripts

```bash
cursdel /tmp/scratch/old-build --json --dry-run
```

produces a single JSON object on stdout instead of the text summary — see
[JSON_OUTPUT.md](JSON_OUTPUT.md) for the full schema, and
[EXIT_CODES.md](EXIT_CODES.md) for the process exit code contract your
scripts can depend on.

## Where to go next

- [COMMAND_REFERENCE.md](COMMAND_REFERENCE.md) — every flag, organized by
  concern.
- [SAFETY_MODEL.md](SAFETY_MODEL.md) — what CurseDelete refuses to delete,
  and why.
- [RETENTION.md](RETENTION.md) and [FILTERS.md](FILTERS.md) — age-based
  cleanup and file filtering in depth.
- [LOCKS.md](LOCKS.md) — dealing with files that are in use.
- [LICENSING.md](LICENSING.md) — editions, `license` subcommand, and what's
  actually technically gated (very little).
