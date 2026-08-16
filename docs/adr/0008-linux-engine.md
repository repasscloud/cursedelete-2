# ADR-0008: Linux native engine

## Status

Accepted.

## Context

`README-CurseDelete2.md` section 6 names the Linux-specific primitives this
engine should use (`openat()`, `unlinkat()`, `fstatat()`, `getdents64()`
"where benchmarking justifies it", `chmod()`/`fchmodat()`), and requires
accounting for Unix permission semantics -- deletion authority comes from
the *parent directory's* write permission, not the object's own mode bits.

`cursdel-macos` (ADR-0006) already establishes this workspace's approach to
POSIX delete operations: `openat`/`unlinkat`-relative removal for TOCTOU
mitigation, a `d_type`-based fast path that skips a metadata syscall for
confidently-identified directories, and modest, parent-directory-focused
permission remediation. Linux and Darwin share enough POSIX surface
(`opendir`/`readdir`/`unlinkat`/`fstatat`/`AT_SYMLINK_NOFOLLOW`/
`AT_REMOVEDIR` all behave identically) that `cursdel-linux` is structurally
the mac engine ported, not redesigned -- this ADR exists to record the
places where the platforms genuinely differ and what was decided for each,
rather than to re-justify decisions ADR-0006 already made once for POSIX in
general.

Three genuine differences needed their own decision:

1. Linux's classic `stat`/`fstatat` has no creation-time ("birth time")
   field at all -- unlike Darwin's `st_birthtime`. `--age-by created` needs
   *some* answer for what Linux reports.
2. The product brief explicitly gates `getdents64()` on "where benchmarking
   justifies it" -- a decision needed making, not assuming.
3. Linux has no Restart Manager equivalent (same as Darwin), but *does*
   have a native mechanism Darwin lacks for identifying local lock holders:
   `/proc/[pid]/fd/*`.

## Decision

### Creation time: `statx` with a `fstatat` fallback, not unconditional `None`

`cursdel-linux::dirlist` calls `statx(2)` (requesting
`STATX_BASIC_STATS | STATX_BTIME` in one call) as the primary metadata
lookup for every non-`DT_DIR` directory entry, in the same position in the
walk where `cursdel-macos::dirlist` calls `fstatat`. This costs no extra
syscall over the macOS engine's one-`fstatat`-per-entry approach -- `statx`
subsumes what `fstatat` would have provided (type, size, mtime, atime) and
additionally reports birth time when the kernel and filesystem support it.

Two layers of graceful degradation, both already safe by construction via
`crate::filter`'s existing handling of unknown timestamps:

- **Per-filesystem:** `statx` reports which requested fields it actually
  populated via `stx_mask`. This is checked explicitly (`stx_mask &
  STATX_BTIME != 0`) rather than assumed from the syscall merely
  succeeding -- ext4 and btrfs populate birth time; tmpfs and some
  network/FUSE filesystems do not, and `stx_mask` says so on a per-call
  basis rather than requiring the caller to hard-code a filesystem
  allowlist.
- **Per-kernel:** if `statx` itself is unavailable (`ENOSYS` -- a
  pre-4.11 kernel, or a container/seccomp profile that blocks the
  syscall), `cursdel-linux::dirlist::lookup_metadata` falls back to plain
  `fstatat`, which loses only creation-time reporting.

In both degradation cases, `created` becomes `None`. `cursdel_core::filter`
already treats a `None` timestamp as `RetainReason::TimestampUnavailable`
(retain, never delete) rather than as "old enough to delete" -- see
`FilterSpec::decide` in `crates/cursdel-core/src/filter.rs`. This makes the
simpler, unconditional-`None` alternative *safe* but not chosen: `statx` is
one syscall, already available via the `libc` crate for both
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` (confirmed by
compiling a probe against both targets), so there is no meaningful
complexity cost for a real capability gain on the filesystems that matter
most (ext4, the default for the large majority of Linux deployments this
tool will run against).

### `opendir`/`readdir`, not raw `getdents64`

`cursdel-linux::dirlist::LinuxDirLister` uses `opendir`/`fdopendir`/
`readdir`/`closedir` from the `libc` crate -- the same call shape as
`cursdel-macos::dirlist`, and the product brief's own default ("native
metadata operations where they improve performance", with `getdents64`
explicitly gated on benchmarking justification, not requested outright).
glibc's `readdir` is itself backed by `getdents64` internally; a raw
`getdents64` syscall wrapper would only pay for itself by removing the
buffering/allocation glibc does around that syscall, which is a real but
unmeasured cost. No benchmark exists yet showing this buffering is a
bottleneck for CurseDelete's workload (dominated by delete syscalls and,
for large trees, storage latency -- not directory-listing parse overhead),
so per engineering rule 9 ("benchmark architectural changes"), the
lower-risk standard library call is what ships. Reaching for the raw
syscall without that evidence would be exactly the kind of speculative
optimisation the product brief's performance principles ("use native
platform APIs where benchmarking proves value") argue against. This is
tracked as a candidate follow-up, not a rejected idea: if `--destroy`
throughput benchmarking against `rm -rf` on very wide, very deep trees ever
shows measurable syscall/parse overhead attributable to `readdir`, revisit
with real numbers.

### `--kill-locks`: native `/proc` scanning, not a subprocess

`cursdel-macos::lock` shells out to `lsof -t <path>` because Darwin has no
public, stable, struct-layout-safe way to enumerate a process's open files
without either an undocumented `libproc` FFI surface or a subprocess.
Linux does not have this constraint: `/proc/[pid]/fd/*` are symlinks to
every file descriptor a process has open, directly readable via
`readlink()` with no special privilege beyond what already gates reading
another user's `/proc/[pid]/fd` directory at all (`EACCES` for processes
this user does not own -- the same boundary `lsof` itself is subject to,
enforced by the kernel, not by either tool).

`cursdel-linux::lock::find_holding_pids` walks `/proc`, filters entries to
numeric PIDs, skips best-effort-detected kernel threads (direct children of
PID 2, `kthreadd`, which cannot hold a userspace file open), and for each
remaining PID reads every `/proc/[pid]/fd/N` symlink target, comparing it
(after stripping the kernel's `" (deleted)"` marker suffix for an
already-unlinked-but-still-open file) against the target path's
canonicalised form.

This is a genuine, not merely cosmetic, improvement over the macOS engine:

- **No subprocess.** No `fork`/`exec`, no dependency on `lsof` being
  installed and on `$PATH` (not guaranteed on minimal container/server
  Linux images, which is exactly where CurseDelete's automation use case
  concentrates), no shell-argument-quoting surface for the path being
  passed as a subprocess argument.
- **No parsing of another program's text output.** `lsof -t` output
  format is a de facto contract, not a documented API; `/proc/[pid]/fd`
  symlink targets are a kernel-guaranteed interface.
- **Same underlying technique, one layer closer to the source.** `lsof`
  and `fuser` both implement this by reading `/proc` themselves on Linux;
  this removes the middleman rather than reinventing something novel.

Protected-PID rules mirror the macOS engine's own-process/PID-1 protection,
extended with the Linux-specific PID 2 (`kthreadd`) case the product brief
calls for: `is_protected_pid` in `cursdel-linux::lock` refuses to signal
PID <= 2 or CurseDelete's own PID. Full kernel-thread ancestry detection
(walking `/proc/[pid]/status`'s `Kthread:` field, not present on all kernel
versions) was judged unnecessary complexity beyond the direct-child-of-PID-2
check already applied at discovery time as a scanning optimisation --
kernel threads structurally cannot hold a userspace fd table entry matching
a regular file path, so even in the (extremely unlikely) case a kernel
thread's PID slipped past discovery-time filtering, it could never appear
in `find_holding_pids`' results in the first place.

`--close-remote-locks` remains Windows-server-specific per the product
brief; `cursdel-linux::lock::resolve_remote_lock` always returns
`LockResolution::Unsupported`, identically to the macOS engine.

### TOCTOU mitigation: same accepted scope as ADR-0006

`cursdel-linux::ops` uses exactly the pattern ADR-0006 establishes: reopen
the target's immediate parent with `O_DIRECTORY | O_NOFOLLOW` immediately
before `unlinkat`, operating on a directory descriptor and a name rather
than re-resolving a path string. This closes the same final-component race
ADR-0006 describes and carries the same accepted residual risk: the
ancestor-chain race (an *earlier* ancestor, several levels up, replaced
with a symlink between when the walk first visited it and when a deletion
deep inside it finally runs) is **not** closed here either, for the same
reason ADR-0006 gives -- `cursdel_core::walk::stream_tree` is platform-
agnostic and operates on `PathBuf`, not file descriptors, so a full
fd-chained walk was judged not worth the cross-platform architecture cost
in this pass, on Linux exactly as on Darwin. This ADR does not claim a
stronger guarantee than ADR-0006 makes; both platforms' residual risk is
tracked together as the same open follow-up.

### A genuine Linux/glibc remediation quirk, noted rather than hidden

`fchmodat(..., AT_SYMLINK_NOFOLLOW)` on a symlink's own name fails with
`ENOTSUP`/`EOPNOTSUPP` on Linux -- glibc has no kernel path to `chmod` a
symlink's own (kernel-ignored, always-`0777`) permission bits, unlike
Darwin, which accepts the flag. `cursdel-linux::ops::remediate` handles
this the same way it handles any other single-call failure within
remediation: the object-level `chmod` attempt simply reports `false` and
the parent-directory `chmod` (the call that actually matters for unlinking
a symlink, since deletion authority comes from the parent directory) is
unaffected, so overall remediation still reports `Applied` when the parent
call succeeds. This is exercised directly by
`ops::tests::remediation_on_a_symlink_still_succeeds_via_parent_chmod`.

## Consequences

- `cursdel-linux` is validated on this macOS development host by
  cross-compilation only (`cargo check`/`cargo clippy --all-targets` for
  both `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, per
  ADR-0001's established pattern) plus the `statx`/`dirent.d_type`/
  `__errno_location` FFI surface being independently confirmed to compile
  against real `libc` 0.2 bindings for both targets. Real filesystem
  behaviour -- in particular, actually observing `STATX_BTIME` populated
  on a live ext4 filesystem, and the `/proc`-based lock scan running
  against real concurrent processes -- can only be confirmed on a real
  Linux CI runner; the affected tests are marked with `// TODO(linux-ci):`
  comments stating exactly what real hardware would add.
- Retention filtering (`--age-by created`) is meaningfully more capable on
  Linux than a "just report `None`" implementation would have been, on the
  filesystems most CurseDelete users will actually run against (ext4,
  btrfs), while degrading safely (never mis-filing an unknown-age file as
  deletable) everywhere else.
- `--kill-locks` on Linux has no external-binary dependency, unlike the
  macOS engine -- a real operational advantage for container/minimal-server
  deployments, which is a meaningful share of CurseDelete's intended
  automation use case per `README-CurseDelete2.md` section 25.3.
- If `--destroy`-mode throughput benchmarking ever identifies
  `readdir`-buffering overhead as a bottleneck on very large trees, the
  `getdents64` decision above is the one to revisit first, with numbers.
