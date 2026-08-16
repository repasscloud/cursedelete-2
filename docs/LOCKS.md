# Locked Files

CurseDelete distinguishes two entirely different kinds of "the file is in
use": a **local** process on the same machine holding it open
(`--kill-locks`), and a **remote** SMB client on a different machine
holding it open through a Windows file server (`--close-remote-locks`).
These require different mechanisms, different privileges, and — for the
remote case — a commercial license, so they are always separate, explicit
flags. Neither is ever implied by the other.

## `--kill-locks`: local process locks

When a delete fails with a sharing/busy violation, `--kill-locks` attempts
to identify the local process holding the file open, terminate it, wait
briefly for the handle to actually release, and retry the delete.
Implied by `--destroy`.

CurseDelete never terminates its own process, and never terminates a
protected system process (PID 1 on POSIX). If every process holding the
file open is protected, the lock is reported as unresolved rather than
CurseDelete terminating something it shouldn't just to force progress.

### Per-platform mechanism

| Platform | Status | Mechanism |
|---|---|---|
| macOS | **Implemented** | Shells out to `lsof -t <path>` to enumerate holding PIDs, then sends `SIGTERM` to each non-protected PID. |
| Windows | **Implemented** | Windows Restart Manager (`RmStartSession`/`RmRegisterResources`/`RmGetList`) identifies holders; since `RmShutdown` only politely asks a process to close via `WM_QUERYENDSESSION` (which most non-GUI/service processes never answer), the engine terminates the identified holder directly via `TerminateProcess`, then waits on the process handle for actual exit before returning. Protects the current process, anything Restart Manager itself flags critical, a system-process name denylist, and (conservatively) any holder whose executable name can't even be resolved. See [ADR-0007](adr/0007-windows-engine.md). Validated by cross-compilation and unit tests; a live second process to terminate needs a real Windows session (`TODO(windows-ci)`). |
| Linux | **Implemented** | Walks `/proc/[pid]/fd/*` natively (no subprocess), reading each descriptor's `readlink()` target and comparing it against the canonicalised target path, then sends `SIGTERM` to each non-protected holding PID. Protects the current process, PID 1, and PID 2 (`kthreadd`); best-effort skips other kernel threads (direct children of PID 2) during the scan itself. See [ADR-0008](adr/0008-linux-engine.md). Validated by cross-compilation and unit tests, including a real end-to-end check of the `/proc` walk against this process's own open file handle; a live *second* process to terminate needs a real Linux session (`TODO(linux-ci)`). |

### Why macOS shells out to `lsof` (and Linux doesn't need to)

Darwin has no equivalent of Windows Restart Manager. The standard way to
identify which process holds a file open is the same query `lsof` itself
performs. Rather than hand-rolling `libproc` FFI against undocumented
struct layouts for a secondary, opt-in feature, the macOS engine
(`crates/cursdel-macos/src/lock.rs`) shells out to `lsof -t <path>`, which
is preinstalled on every macOS system and is the de facto standard tool for
exactly this query. A native `libproc`-based implementation (removing the
subprocess dependency) is recorded as documented follow-up work in
[ADR-0006](adr/0006-posix-toctou.md) rather than implemented now.

`lsof` exiting non-zero because nothing has the file open is treated as a
valid, empty result, not an error — only a genuinely unrunnable `lsof` (not
installed, permission denied) is reported as the lock being unresolvable.

Linux does not need this workaround: `/proc/[pid]/fd/*` are symlinks to
every file descriptor a process has open, readable directly with
`readlink()` and no external tool involved — the same technique `lsof`/
`fuser` use internally on Linux, just without the subprocess, the
dependency on an external binary being installed and on `$PATH` (not
guaranteed on minimal container/server images), or the risk of parsing
another program's undocumented text-output format. `EACCES` reading
another user's `/proc/[pid]/fd` (expected — introspecting a process you
don't own requires privilege you may not have) and `ENOENT` (the process
exited mid-scan) are both treated as "skip this PID," not scan failures;
only `/proc` itself being unreadable is reported as the mechanism being
unavailable. See [ADR-0008](adr/0008-linux-engine.md) for the full
comparison against the macOS approach.

### Verified failure behavior when disabled

```console
$ # --kill-locks not passed, delete fails with a sharing violation
Error category: sharing_violation
(local lock resolution not attempted — not enabled)
```

If a sharing violation occurs and `--kill-locks` was never passed,
CurseDelete reports the failure as-is; it does not attempt any lock
resolution unless you explicitly asked for it.

## `--close-remote-locks`: remote SMB opens

A process on a *different* machine holding a file open through a Windows
file server is not something `--kill-locks` can ever resolve — terminating
processes on the machine running `cursdel` does nothing to a session held
open by a different computer against the file server. `--close-remote-locks`
is a distinct, higher-risk capability that administratively closes the
matching remote SMB open on the server itself, then retries the delete.

```bash
cursdel \\FS01\Builds\Old --force --close-remote-locks
```

### Requirements, all of which must hold

1. **Windows file server only.** This capability only exists for
   Windows-hosted SMB shares. It does not apply to, and will never be
   claimed to work for, Samba, NetApp, Synology, third-party NAS
   appliances, or cloud SMB implementations — closing a remote open
   requires calling that specific server's own remote-management surface,
   which non-Windows servers don't expose in a compatible way.
2. **Suitable administrative rights on the file server itself.** Local
   administrator rights on the machine running `cursdel` do **not** imply
   remote administrative rights on the file server — these are separate
   authorization boundaries and CurseDelete never assumes otherwise.
3. **Business or Enterprise license.** This is the one capability
   CurseDelete's edition policy technically gates — see
   [LICENSING.md](LICENSING.md#what-is-actually-technically-gated). Without
   a qualifying license, the flag is rejected before any operation begins:

   ```console
   $ cursdel /tmp --close-remote-locks
   Error: --close-remote-locks requires a Business or Enterprise licence.
   Run 'cursdel license status' for details, or 'cursdel license activate'
   to activate one.
   $ echo $?
   7
   ```

4. **Explicit opt-in, every time.** `--close-remote-locks` is never implied
   by `--force` or `--destroy`. Administratively closing another user's
   (or application's) open file handle can cause data loss for whatever
   was using it, so it always requires the operator to ask for it by name
   on that specific invocation.

### Current implementation status

The Windows engine (`cursdel-windows`) implements remote SMB open closure
via `NetFileEnum`/`NetFileClose` against the UNC path's server: it parses
the server and share-relative path out of the UNC target, enumerates that
server's open files, matches by comparing the trailing path components of
each open file's local path against the requested share-relative path
(a sound comparison — a share always maps its root to some local path, so
everything below the share root has identical relative structure on both
views, not a heuristic guess), and calls `NetFileClose` on every match.
Any non-UNC path, or any `NetFileEnum`/`NetFileClose` failure (access
denied on the remote server, RPC unreachable, the server doesn't
implement this API at all — Samba, NetApp, Synology, ...), is reported as
`Unsupported` with an honest message, never a faked success. This still
needs validation against a real file server (`TODO(windows-ci)` in
`crates/cursdel-windows/src/lock.rs`) — see
[ADR-0007](adr/0007-windows-engine.md).

On macOS and Linux, `--close-remote-locks` is always reported unsupported,
per the product's Windows-server-only requirement:

```console
# from cursdel-macos/src/lock.rs::resolve_remote_lock, verified by its own test suite
"remote SMB open administration is only implemented for supported Windows
file servers"
```

This is intentional, not a bug to work around: CurseDelete never fakes
success on a server/platform combination it cannot actually act on. A
failed or unsupported remote-lock attempt is reported as a
`remote_lock_unsupported` or `remote_lock_failed` failure category (see
[JSON_OUTPUT.md](JSON_OUTPUT.md#failures-entries)) and drives the
`RemoteLockHandlingFailed` (5) exit code (see [EXIT_CODES.md](EXIT_CODES.md))
— it is never silently swallowed or reported as if the delete simply
succeeded.
