# Retention Cleanup

Age-based deletion — "remove files older than N days, leave the rest" — is a
first-class use case, not a bolt-on filter. This document covers `--age`
and `--age-by` syntax, directory cleanup semantics, and per-platform
timestamp reality (which is not the same as the ideal — see
[the limitations section](#per-platform-timestamp-availability)).

## `--age`: duration syntax

```bash
cursdel Logs --age 2d
cursdel /var/cache/app --age 12h
cursdel \\FS01\Logs --age 90d
```

A unit is **mandatory**. `--age 2` is rejected:

```console
$ cursdel /tmp --age 2
Error: '--age 2' is missing a unit. A duration unit is mandatory: use m
(minutes), h (hours), d (days), or w (weeks), e.g. --age 2d
```

This is deliberate, not an oversight: "2" is genuinely ambiguous between
two minutes and two weeks, and a destructive tool getting that wrong by a
factor of thousands is a real, not theoretical, risk. The parse error tells
you exactly what's missing rather than guessing at a default unit.

### Accepted units

Exactly (from [`crates/cursdel-core/src/duration.rs`](../crates/cursdel-core/src/duration.rs)),
case-insensitive:

| Unit | Spellings | Seconds |
|---|---|---:|
| minutes | `m`, `min`, `mins`, `minute`, `minutes` | 60 |
| hours | `h`, `hr`, `hrs`, `hour`, `hours` | 3,600 |
| days | `d`, `day`, `days` | 86,400 |
| weeks | `w`, `wk`, `wks`, `week`, `weeks` | 604,800 |

There is no `s` (seconds) or month/year unit. Fractional values are
accepted (`--age 1.5h`), and the value must be strictly positive — `0d` and
any negative value are both rejected.

### Age boundary

A file exactly at the threshold qualifies for deletion: "at least this
old" is inclusive (`elapsed >= age`, verified directly in
`filter.rs::age_boundary_exact_threshold_is_deleted`). A file one second
younger than the threshold is retained.

## `--age-by <modified|created|accessed>`

Default: `modified`. Selects which timestamp `--age` measures against.

- **`modified`** (default) — the file's content-modification time. This is
  the timestamp every filesystem CurseDelete supports reports reliably,
  which is why it's the default.
- **`created`** — file creation/"birth" time, where the filesystem and OS
  report one. See platform availability below.
- **`accessed`** — last-read time. See the `noatime`/`relatime` caveat
  below — this is the least reliable basis in practice.

If the selected timestamp is unavailable for a given file (platform or
filesystem doesn't report it), that file is **retained**, never deleted on
an unverifiable basis (`filter.rs::missing_timestamp_is_retained_not_deleted`).
CurseDelete never treats "unknown age" as "old enough to delete."

## Per-platform timestamp availability

This section describes what each platform engine actually reports today,
not what would be ideal.

### macOS (implemented)

The macOS engine (`crates/cursdel-macos/src/dirlist.rs`) populates all
three timestamps from a single `fstatat` call per file: `modified` from
`st_mtime`, `accessed` from `st_atime`, and — notably — `created` from
`st_birthtime`, which APFS and HFS+ both support natively. On macOS,
`--age-by created` is fully reliable for files on a supported local
filesystem; there is no fallback or unavailability case to document here
for the standard case.

`--age-by accessed` still carries the general caveat below: macOS does
update `atime`, but any Unix-style access-time semantics are still
best-effort and can be affected by mount options or the specific
filesystem/volume in use — don't build a policy around sub-second accuracy
of "last read."

### Linux (implemented)

The Linux engine (`crates/cursdel-linux/src/dirlist.rs`) calls `statx(2)`
(requesting `STATX_BASIC_STATS | STATX_BTIME` in one syscall) rather than
classic `fstatat`, specifically because classic `stat`/`fstatat` has no
creation-time field on Linux at all — `statx` is the only way to ask for
one. Whether `created` actually comes back populated depends on the
underlying filesystem and kernel, and this engine checks that explicitly
per call (via `statx`'s `stx_mask` result) rather than assuming it:

- **ext4 and btrfs** populate birth time; `--age-by created` works as
  expected on the filesystems most Linux deployments actually use.
- **tmpfs and some network/FUSE filesystems** do not report a birth time
  even though the `statx` call itself succeeds — `created` is `None` for
  files there, so those files are **retained** under `--age-by created`
  (per the unavailable-timestamp rule above), never silently deleted on a
  guessed or substituted timestamp.
- **Pre-4.11 kernels, or a container/seccomp profile that blocks `statx`
  entirely** — the engine falls back to plain `fstatat`, which reports
  `modified` and `accessed` normally but `created` as `None` unconditionally
  (there is no fallback source for creation time to fall back *to* on
  Linux).

See [ADR-0008](adr/0008-linux-engine.md) for the full reasoning. The
`STATX_BTIME`-populated path is validated by cross-compilation and unit
tests that check internal consistency (creation time never reported after
modification time); actually observing `STATX_BTIME` set on a live ext4
volume is one of the `TODO(linux-ci)` items pending validation on real
Linux hardware — the test suite deliberately does not assert `created` is
always `Some`, since that would make the test fail on any CI runner backed
by a filesystem that doesn't populate it (tmpfs, `/tmp` on some
containers).

`--age-by accessed` carries the general caveat below, plus Linux is the
platform where it applies most often in practice: `relatime` (the common
Linux default) only updates access time once per day or on writes, and
`noatime` disables the update entirely.

### Windows (implemented)

`crates/cursdel-windows/src/dirlist.rs` populates all three timestamps
(`modified`, `created`, `accessed`) directly from the
`ftLastWriteTime`/`ftCreationTime`/`ftLastAccessTime` fields
`FindFirstFileExW` already returns as part of enumeration — no extra
per-file syscall. NTFS/ReFS reliably track a real creation time, so
`--age-by created` is expected to work correctly; `accessed` still carries
the general last-access-time caveat below (Windows disables last-access-
time updates by default on modern systems for performance, similar in
spirit to Linux's `relatime`). Real-filesystem confirmation of exact
timestamp behaviour is one of the `TODO(windows-ci)` items pending a live
Windows session — the field mapping itself is implemented and unit-tested
for the `FILETIME`→`SystemTime` conversion (100ns intervals since
1601-01-01, converted precisely, not approximated) in
`crates/cursdel-windows/src/sys.rs`.

### The `accessed`/`noatime` caveat (all platforms)

`--age-by accessed` is inherently the least trustworthy basis for a
retention policy on any POSIX system. Many production Linux (and some
macOS/network-filesystem) deployments mount filesystems with `noatime` or
`relatime` for performance reasons — `noatime` means access time is never
updated at all, and `relatime` (the common Linux default) only updates it
once per day or on writes, not on every read. A file "read yesterday" may
still show an access time from weeks ago under `relatime`, and under
`noatime` it may never move from whatever it was at creation. If you're
building an automation policy around "delete anything not accessed in 90
days," verify your actual mount options first — `modified` (the default)
is almost always the more predictable choice unless you have a specific,
verified need for access-time semantics.

## Directory behavior

With `--age` (or any other filter), the filter applies to files only.
Directories are never matched directly — they are removed once they become
empty after their qualifying children are processed, and retained
otherwise. See [SAFETY_MODEL.md](SAFETY_MODEL.md#retention-modes-directory-root-preservation-rule)
for why the *target* directory itself is never removed under a filtered
operation even if it ends up empty.

### Worked example

Starting tree, with `old.log` and `Ancient/old.log` backdated well past the
threshold and everything else current:

```text
Logs/
├── old.log          (old)
├── yesterday.log    (recent)
├── Ancient/
│   └── old.log      (old)
├── Current/
│   └── today.log    (recent)
└── Empty/
```

Running `cursdel Logs --age 2d` against this tree (verified output, real
run):

```console
$ cursdel Logs --age 2d
CurseDelete 2

Target:       .../Logs
Mode:         normal
Workers:      auto -> 10
ACL repair:   disabled
Kill locks:   disabled
Remote locks: disabled

Files:          2
Directories:    2
Retained:       2 files, 1 directories
Deleted:        10.0 B
Failures:       0
Elapsed:        00:00:00
Rate:           9 files/sec

Complete.
```

Resulting tree:

```text
Logs/
├── yesterday.log
└── Current/
    └── today.log
```

- `old.log` and `Ancient/old.log` qualified for deletion (2 files deleted).
- `Ancient/` disappears (2 directories deleted: `Ancient` and `Empty`) —
  once its only child was deleted, it became empty and was removed in its
  own right.
- `Empty/` disappears because it was already empty.
- `yesterday.log` and `Current/today.log` are retained (2 files retained)
  because they don't qualify by age.
- `Current/` is retained (1 directory retained) because it still contains
  `today.log`.
- `Logs/` itself is **never a candidate for deletion** under a filtered
  run, regardless of what ends up inside it — see
  [SAFETY_MODEL.md](SAFETY_MODEL.md#retention-modes-directory-root-preservation-rule).

Newer files are never deleted merely to let a parent directory become
empty — directory removal is a *consequence* of every child qualifying, not
a goal the filter is allowed to work backward from.

## Preview before you run for real

Every retention run should start with `--dry-run`:

```bash
cursdel Logs --age 90d --dry-run
```

This reports exactly what would be deleted and retained — see
[QUICKSTART.md](QUICKSTART.md) and [JSON_OUTPUT.md](JSON_OUTPUT.md) for the
exact output shape — without touching the filesystem.
