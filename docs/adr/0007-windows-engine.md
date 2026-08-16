# ADR-0007: Windows native engine

## Status

Accepted.

## Context

`README-CurseDelete2.md` section 5 names the Windows-specific primitives
this engine must use (`FindFirstFileExW`/`FIND_FIRST_EX_LARGE_FETCH`,
`FILE_DISPOSITION_INFO_EX`, `FILE_FLAG_OPEN_REPARSE_POINT`,
`AdjustTokenPrivileges`, `SetNamedSecurityInfoW`), and sections 5.1-5.3,
12.1, and 13 describe the required behaviour for permission/ACL
remediation, long paths/UNC, reparse-point safety, local lock handling,
and remote SMB lock handling respectively.

`_old/sfvdd` already validated most of this API surface works (see its
`win_delete.rs`): the modern disposition-based delete call, long-path
prefixing, ownership/ACL remediation, and privilege enablement. It is not
reused wholesale for two reasons documented in ADR-0001: its "fast" path
collects the entire tree into `Vec`s before deleting anything (the exact
anti-pattern `cursdel_core::walk::stream_tree` exists to avoid), and
several of its error-handling shortcuts are not acceptable in a supported
product (see "Fallback error handling" below).

`cursdel-macos` (ADR-0006) already establishes this workspace's approach
to TOCTOU mitigation on delete operations and the honesty standard for
documenting residual risk. This ADR is the Windows-engine analogue, using
Windows' different (handle-based, not parent-relative-unlink-based)
security model.

## Decision

### Long-path and UNC prefixing

Every path handed to a Win32 filesystem call (`FindFirstFileExW`,
`CreateFileW`, `DeleteFileW`, `RemoveDirectoryW`, `GetFileAttributesW`,
`SetFileAttributesW`, `SetNamedSecurityInfoW`) goes through
`sys::to_verbatim`, unconditionally, not only when the path happens to
exceed `MAX_PATH`:

1. If the path is relative (should not normally occur --
   `CanonicalTarget::canonical` is already absolute, and
   `CanonicalTarget::requested`, used for single-object deletes via
   `cursdel_core::pipeline::run_single_object`, is whatever the CLI
   argument was), it is lexically joined onto the current directory --
   `std::env::current_dir().join(path)`, never `std::fs::canonicalize`,
   which would resolve symlinks and could silently redirect the delete
   target.
2. Forward slashes are normalised to backslashes. This step is easy to
   miss: the `\\?\` verbatim prefix disables Win32's own path
   normalisation, so a path that would otherwise tolerate `/` (e.g. a user
   typing `cursdel C:/Temp/Old`) does not once it is verbatim-prefixed,
   and would fail with `ERROR_INVALID_NAME` instead.
3. `\\?\` is prepended for a drive path, `\\?\UNC\` for a `\\server\share\...`
   path; an already-verbatim path (the common case -- `std::fs::canonicalize`
   on Windows returns `\\?\`-prefixed paths, and `CanonicalTarget::canonical`
   is exactly that) is returned unchanged.

This is applied unconditionally rather than conditionally on path length
for a reason beyond long-path support: the verbatim prefix also disables
`.`/`..` reinterpretation and other Win32 path normalisation quirks, so
the exact path string the tree walker discovered is the exact string every
delete syscall receives, with nothing in between able to reinterpret it.

`sys::to_verbatim`/`to_wide_verbatim` are pure functions covered by unit
tests for every prefix combination (drive, UNC, already-verbatim in both
forms, mixed separators). They cannot be exercised end-to-end against a
real long path or a real UNC share on this development machine (macOS);
see the `TODO(windows-ci)` in `lib.rs`.

### Delete strategy: `FILE_DISPOSITION_INFO_EX` primary, classic fallback

`ops::delete_file`/`delete_dir` open the target with `CreateFileW`
(`DELETE | FILE_READ_ATTRIBUTES`, all three share flags, `OPEN_EXISTING`)
and call `SetFileInformationByHandle(FileDispositionInfoEx,
FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS |
FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE)`. This is preferred over
the classic APIs for two reasons: `POSIX_SEMANTICS` removes the directory
entry immediately rather than deferring to last-handle-close (matching
what the rest of the pipeline assumes -- a delete either succeeded or it
did not), and `IGNORE_READONLY_ATTRIBUTE` means a plain, non-`--force`
delete already succeeds against a read-only file without a separate
attribute-clearing round trip, which keeps the fast path fast (product
principle: "do not read metadata unless the operation requires it" --
`remediate` is simply never invoked for the common read-only-file case).

If opening the handle fails with `ERROR_FILE_NOT_FOUND`/`ERROR_PATH_NOT_FOUND`,
the result is `AlreadyGone` immediately (benign race with a concurrent
process), with no further attempt. For every other failure of the
handle-based path -- opening or setting the disposition -- `ops` falls
back to `DeleteFileW`/`RemoveDirectoryW`, unconditionally rather than
branching on a specific error code. This is deliberate: the handle-based
path can fail for the same underlying reason the classic API would (in
which case the fallback fails identically and reports an equally accurate
error), or for a reason specific to the newer API (older filesystem
driver, third-party filter driver that does not understand
`FileDispositionInfoEx`, etc.), in which case the fallback is exactly the
right thing to try. The primary path's own error is logged at `trace`
level for diagnostics and otherwise discarded; **the fallback's own
result -- success or failure -- is always what gets reported to the
caller.**

This is a deliberate divergence from `_old/sfvdd`, which discards the
fallback's result entirely (`let _ = DeleteFileW(pcw(path));`, ignoring
whether it succeeded) and always reports the *original* error, or
silently reports success regardless of what actually happened. That
pattern is not acceptable here: engineering rule 17 ("never hide failed
deletions") means the fallback's real outcome must be surfaced.

### Reparse-point safety: unconditional `FILE_FLAG_OPEN_REPARSE_POINT`

Every `CreateFileW` call in `ops.rs` passes `FILE_FLAG_OPEN_REPARSE_POINT`
unconditionally -- not only when a prior attribute check reports the
target is a reparse point. This is a deliberate improvement over
`_old/sfvdd`'s pattern (`GetFileAttributesW` first, then conditionally add
the flag), which has a check-then-open race: the object could become a
reparse point between the attribute check and the `CreateFileW` call.
Since `FILE_FLAG_OPEN_REPARSE_POINT` is harmless when the target is *not*
a reparse point (Win32 simply opens the object normally), there is no
downside to setting it unconditionally, and doing so removes that race
entirely for the final path component.

Directories are additionally opened with `FILE_FLAG_BACKUP_SEMANTICS`
(required to obtain a directory handle at all; also harmless for a
directory reparse point/junction combined with `OPEN_REPARSE_POINT`).

**What this does and does not protect against**, mirroring ADR-0006's
honesty about residual risk for the POSIX engines:

- **Protected**: the final path component. If the object CurseDelete is
  about to delete is, or has been swapped for, a symlink/junction/reparse
  point, `CreateFileW` opens the link object itself and the subsequent
  `FileDispositionInfoEx`/`DeleteFileW`/`RemoveDirectoryW` call removes
  the link, never whatever it points to. This is what makes "delete
  `C:\DeleteMe\junction`" safe even if `junction` points at
  `D:\ImportantData` (`README-CurseDelete2.md` section 5.3's example).
- **Not protected**: the **ancestor chain**. Every path this crate hands
  to a Win32 call is a `PathBuf` re-resolved from the root on every call
  (via `to_verbatim`), not a chain of already-open directory handles the
  way `cursdel-macos`'s `openat`-relative approach partially achieves (see
  ADR-0006). Windows has an analogous primitive to close this gap --
  `NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory` set to an
  already-open parent handle, used at every level of the walk -- but
  `cursdel_core::walk::stream_tree` is intentionally platform-agnostic and
  operates on `PathBuf`, not OS handles (ADR-0001), so threading a
  handle-chain through the shared walker was judged out of scope for this
  pass, exactly as ADR-0006 records for the POSIX engines' equivalent gap.
  A local attacker with write access to an already-visited ancestor
  directory inside the target tree, and the ability to predict traversal
  timing, could in principle redirect a deeply-nested delete between
  discovery and the delete call. This requires local code execution with
  write access inside the tree being deleted -- a narrower threat model
  than remote or unauthenticated attack, and CurseDelete is an
  administrator-invoked tool typically run against trees the operator
  already controls.
- **Directory listing** (`dirlist.rs`) reports raw attributes
  (`FILE_ATTRIBUTE_REPARSE_POINT`/`FILE_ATTRIBUTE_DIRECTORY`) exactly as
  `FindFirstFileExW` returns them for each entry, with no follow-up call;
  `cursdel_core::walk::stream_tree` never pushes a reparse point onto its
  traversal stack (ADR-0005), so this crate is never asked to list the
  target of a junction/symlink in the first place.

### Attribute/ownership/ACL remediation

`ops::remediate` is only invoked by `cursdel-core`'s pipeline when
`DeleteOptions.allow_remediation` is set and the failure category is
`AccessDenied` or `ReparsePointRefused` (`pipeline::is_remediable`) --
never on the successful path, satisfying "do not perform expensive
remediation on the normal successful path."

It performs two steps in a single call:

1. Clear `FILE_ATTRIBUTE_READONLY` if set (`SetFileAttributesW`).
   Deliberately does **not** touch hidden/system attributes: neither
   actually blocks a Win32 delete (only read-only does), so clearing them
   would be an unrequested side effect with no benefit if the object
   ultimately survives (remediation as a whole can still fail for an
   unrelated ACL reason).
2. Take ownership of, and grant full control (`GENERIC_ALL`) to, **the
   current process token's user** -- resolved via
   `GetTokenInformation(TokenUser)`, never an unconditional
   `BUILTIN\Administrators` SID -- via a single `SetNamedSecurityInfoW`
   call (`OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION |
   PROTECTED_DACL_SECURITY_INFORMATION`). This is the one point where this
   crate's behaviour intentionally differs from `_old/sfvdd`'s
   `take_ownership_and_grant_admins`, per the product brief's exact
   wording in section 5.1: CurseDelete resolves permission/ownership/ACL
   restrictions "where the executing security context has sufficient
   authority to do so" -- not by escalating to a fixed well-known group
   the operator may not actually belong to, and not by claiming a right
   the token does not hold.

Both steps are attempted together, rather than gated on each other's
success, because `cursdel_core::pipeline::process_one` calls `remediate`
exactly once per failed delete and retries the delete exactly once
afterward (see `pipeline.rs`) -- there is no second remediation attempt
available if the first, lighter step alone turns out to be insufficient.
Attempting both together is safe specifically because this only ever runs
on the already-failed path (never the normal successful-delete path), and
each step is independently a no-op when it has nothing to fix, or no
authority to fix it.

**The security boundary is enforced by the OS, not by this code.** If the
current token genuinely lacks `SeTakeOwnershipPrivilege`/`WRITE_OWNER`/
`WRITE_DAC` rights over the object, `SetEntriesInAclW`/
`SetNamedSecurityInfoW` simply fail with a real Win32 error, which is
surfaced as `RemediationOutcome::Failed`, never silently swallowed or
reinterpreted as success. `SE_BACKUP_NAME`/`SE_RESTORE_NAME`/
`SE_TAKE_OWNERSHIP_NAME` are enabled once, best-effort, at engine
construction (`WindowsEngine::new` -> `sys::enable_privileges_best_effort`);
`AdjustTokenPrivileges` reports the ordinary "not elevated" case via
`ERROR_NOT_ALL_ASSIGNED` rather than a hard failure, which is logged at
`debug` level and does not fail engine construction -- plain,
unprivileged deletion (and ACL-permitted remediation without elevation)
must keep working regardless of whether the process is elevated.

### `--kill-locks`: Restart Manager, then direct termination

`lock::resolve_local_lock` uses the Restart Manager API
(`RmStartSession`/`RmRegisterResources`/`RmGetList`) to identify processes
holding the target open, per the product brief's explicit preference
("Preferred first implementation: Windows Restart Manager API").

Restart Manager's own graceful-shutdown mechanism, `RmShutdown`, only
*asks* a process to close via `WM_QUERYENDSESSION` -- a message most
non-GUI processes and virtually all services never answer. This engine
terminates the identified holder directly via `OpenProcess`/
`TerminateProcess` instead, which `README-CurseDelete2.md` section 12.1's
own conceptual flow diagram supports (it goes straight from "identify lock
holder" to "terminate process", with no `RmShutdown` step drawn in). This
is recorded here as the documented interpretation the product brief asks
for.

Before terminating, a holder is skipped (never touched) if any of:

- its PID equals `GetCurrentProcessId()` (CurseDelete never terminates
  itself);
- Restart Manager itself classifies it `RmCritical` -- Restart Manager's
  own signal that the process is essential to the OS, a stronger and more
  authoritative source than any hardcoded list this crate could maintain;
- its resolved executable basename (`QueryFullProcessImageNameW`, not
  Restart Manager's `strAppName`, which is a human-friendly display name
  and not reliably the actual file name) case-insensitively matches the
  denylist `csrss.exe`, `wininit.exe`, `winlogon.exe`, `services.exe`,
  `lsass.exe`, `smss.exe`, `system`;
- **the executable name could not be resolved at all** (`OpenProcess`
  with `PROCESS_QUERY_LIMITED_INFORMATION` or
  `QueryFullProcessImageNameW` itself failed). This is a conservative
  default: an unidentified process is treated as protected rather than
  risking termination of something this engine cannot even name. The
  practical cost is that a legitimate, terminable lock holder running
  under a different, less-privileged security context might be
  incorrectly skipped in rare cases; the alternative (terminating
  processes this engine cannot identify) was judged the wrong trade-off
  for a destructive tool.

After termination, `WaitForSingleObject` on the (still-open,
`SYNCHRONIZE`-rights) process handle polls, bounded to one second per
terminated process, for the process to actually exit -- which is exactly
when the OS releases every handle it held, including the one blocking the
delete -- so the caller's retry has a real chance of succeeding rather
than racing a process that is still tearing down.

`resolve_local_lock` returns `Failed` (never fakes `Resolved`) if: no
holder could be identified at all; every holder is protected; or
termination did not succeed for any non-protected holder.

### `--close-remote-locks`: `NetFileEnum`/`NetFileClose`

`lock::resolve_remote_lock` only applies to UNC paths
(`\\server\share\...` or its verbatim `\\?\UNC\server\share\...` form);
any other path is `Unsupported` immediately. The server name is the first
UNC component; `NetFileEnum` (level 3) is called against that server with
no `basepath` filter (`NULL`), returning every open file the caller's
credentials are authorised to see, and the results are matched
client-side against the requested path by comparing trailing path
components case-insensitively (`path_ends_with_components`) rather than a
raw substring match (which would accept a false match like `ub\file.txt`
against `.../xsub/file.txt`).

Filtering client-side rather than passing a `basepath` to `NetFileEnum`
is deliberate: `basepath` filters by the **server's local filesystem
path**, which this crate has no reliable way to know from a UNC path
alone (mapping a share name to its local path requires `NetShareGetInfo`,
a *separate* administrative query with its own rights requirement). The
client-side comparison is not a heuristic approximation of this: because
a share root always maps to some local path, and everything below the
share root has the *identical* relative structure in both the UNC view
and the server's local view, comparing whether the server's local path
ends with the UNC path's share-relative subpath is a sound equality
check, not a guess.

Matching entries are closed via `NetFileClose`. Exactly per the product
brief's requirement ("do not fake successful remediation"):

- A non-UNC path, or any `NetFileEnum` failure (access denied on the
  remote server, RPC unreachable, or the server simply does not implement
  this API -- Samba, NetApp, Synology, and most third-party NAS/cloud SMB
  implementations either refuse it or do not expose it at all) is
  `Unsupported`, with the underlying Win32 error code in the message.
  Local Administrator rights on the machine running CurseDelete grant
  **nothing** here -- this requires real administrative rights on the
  remote server itself, exactly as section 5.1 warns generally.
- `NetFileEnum` succeeding but finding no path match is `Failed`, not
  `Unsupported`: the mechanism works on this server, but this specific
  open could not be identified (already closed by the time of the query,
  or a path-matching edge case). This distinction matters downstream --
  `pipeline.rs` maps `Unsupported` to `FailureCategory::RemoteLockUnsupported`
  and `Failed` to `FailureCategory::RemoteLockFailed`, which are reported
  differently to the operator.
- `NetFileClose` failing for every matched entry is `Failed` with every
  individual error code collected, not a generic message.

## Consequences

- The fast path (plain delete, no `--force`) never pays for attribute
  inspection, ownership queries, or ACL calls -- those only run after a
  delete has already failed, matching engineering rules 1 and 6.
- Plain deletion works fully unprivileged; `--force`/`--destroy`
  remediation works exactly as far as the executing token's real
  authority extends and no further, satisfying "never claim to bypass
  Windows security boundaries" (section 22) -- the OS enforces this, this
  code only reports what the OS decided.
- `--kill-locks` cannot terminate CurseDelete itself, any process Restart
  Manager itself flags critical, any process on the fixed name denylist,
  or any process this engine could not identify by name -- satisfying
  "critical/system processes must be protected by policy" conservatively
  rather than permissively.
- `--close-remote-locks` never reports success on a server it could not
  actually administer, satisfying "report the remote sharing/locking
  problem correctly; do not fake successful remediation."
- The residual TOCTOU risk on the ancestor chain (not the final
  component, which is fully protected) is accepted and documented here,
  matching ADR-0006's treatment of the structurally similar POSIX gap;
  closing it fully is tracked as follow-up hardening (an `NtCreateFile`/
  `OBJECT_ATTRIBUTES.RootDirectory`-based handle chain), not implemented
  in this pass.
- Everything in this ADR that depends on live Windows behaviour --
  junction/symlink deletion, restrictive-ACL remediation actually
  unblocking a retry, Restart Manager identifying a genuine second
  process's lock, and any `NetFileEnum`/`NetFileClose` behaviour against
  a real file server -- is marked `TODO(windows-ci)` in the corresponding
  test module and has only been validated by cross-compilation
  (`cargo check`/`clippy` for `x86_64-pc-windows-msvc` and
  `x86_64-pc-windows-gnu`) on this (macOS) development machine, not by
  execution. A GitHub Actions Windows runner is the intended place these
  get exercised for real.
