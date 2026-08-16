# ADR-0006: TOCTOU mitigation on POSIX delete operations

## Status

Accepted, with documented residual risk.

## Context

`_old/sfvdd`'s Windows engine and `_old/CurseDelete`'s cross-platform code
both operate purely on path strings: list a directory, then later call
`DeleteFileW`/`File.Delete` on the full path again. Between the listing and
the delete, a local attacker with write access to an ancestor directory can
replace an entry with a symlink, causing the delete to follow the attacker's
link instead of the object that was actually listed -- the classic `rm -rf`
race (CVE-class: untrusted directory swapped for a symlink mid-traversal).
The security review explicitly calls out "TOCTOU issues" and "symlink/
reparse attacks" as required review areas, so this needed a real answer,
not just the existing "don't recurse into discovered symlinks" rule (ADR-0005),
which protects against a *listed* symlink being followed but does nothing
about an ancestor being *replaced* between listing and delete.

The product brief explicitly names the right primitives for this:
`openat()`, `unlinkat()`, `fstatat()`.

## Decision

For each delete (`cursdel-macos::ops`, and the equivalent in
`cursdel-linux`):

1. Re-open the target's **immediate parent** directory with
   `O_DIRECTORY | O_NOFOLLOW`, getting a fresh file descriptor.
   `O_NOFOLLOW` means if that parent path's final component has been
   replaced with a symlink since it was validated, the open fails with
   `ELOOP` instead of silently following it.
2. Call `unlinkat(parent_fd, name, flags)` (or `fstatat`, for metadata) by
   **name**, not by re-resolving the full path string. The kernel performs
   the removal atomically relative to that specific, already-open
   directory -- there is no window between "identify the parent" and
   "remove the child" where the *parent* could be swapped, because the
   parent is already an open descriptor, not a path being re-walked.
3. Directory listing (`cursdel-macos::dirlist`) uses the same
   `open(O_NOFOLLOW) -> fdopendir -> readdir` pattern, and reads metadata
   via `fstatat(dir_fd, name, AT_SYMLINK_NOFOLLOW)` relative to that same
   descriptor rather than a second `lstat(full_path)` call.

### What this does not fix

This closes the **final-component** race (the parent named in the delete
call cannot have been swapped for something else between being opened and
the `unlinkat` call) but does **not** close the **ancestor-chain** race: the
directory descriptor for the immediate parent is obtained by re-resolving
its path from the walk's cached `PathBuf`, which means an *earlier*
ancestor (several levels up, already-visited during the walk) could
theoretically be replaced with a symlink between when the walk first
visited it and when a delete deep inside it finally runs, redirecting that
`open()` call elsewhere. Fully eliminating this would require never
re-resolving any path from the root at all -- keeping every ancestor
directory's descriptor open for the lifetime of the walk and using
`openat` relative to the *parent's already-open fd* at every level, all
the way down. `cursdel-core::walk::stream_tree` is intentionally
platform-agnostic and operates on `PathBuf`, not OS file descriptors (a
fd-based walk doesn't translate to Windows' handle model in the same
shape), so threading a full fd-chain through the shared walker was judged
not to be worth the cross-platform architecture cost for this pass.

## Consequences

- **Residual risk (accepted):** a sustained local attacker who can predict
  CurseDelete's traversal timing and has write access to an ancestor
  directory within the target tree could redirect a deeply-nested delete.
  This requires local code execution with write access inside the target
  tree, and is a materially narrower threat model than an unauthenticated
  or remote attack -- CurseDelete is an administrator-invoked tool typically
  run against trees the operator already controls.
- **Follow-up hardening** (tracked, not yet implemented): thread an
  optional platform-specific "parent handle" through `DirLister`/
  `RawChild` so POSIX engines can keep every ancestor's fd open for the
  duration of the walk (`openat` chained from the root), closing the
  ancestor-chain gap entirely. Windows has an analogous mechanism
  (`NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory` set to a parent
  handle) that the Windows engine does not currently use either -- see
  `docs/adr/0007-windows-engine.md` for the Windows engine's own,
  differently-shaped residual risk in the same area.
- **This follow-up is also a measured performance opportunity, not just a
  security one.** `docs/BENCHMARKS.md` shows CurseDelete meaningfully
  slower than `rm -rf` specifically on deep, narrow trees (0.21x-0.43x
  throughput at 50 directory levels, vs. 0.98x-1.15x on shallow/wide
  trees of the same file count) -- the *same* per-delete "reopen the
  parent by full path" pattern that provides the TOCTOU mitigation also
  costs a full path resolution proportional to depth on every single
  delete, where `rm -rf`'s own implementation opens each directory once
  and reuses that descriptor for every child in it. Fixing the security
  gap and fixing this performance gap are the same code change.
- This is strictly stronger than both prior implementations, which had no
  TOCTOU mitigation at all.
