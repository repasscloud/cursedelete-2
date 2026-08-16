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
| Linux | Planned, not yet implemented | The `cursdel-linux` engine is currently a stub — see [README.md](../README.md#platform-support). A `/proc`-based holder search (scanning `/proc/*/fd` symlinks) is the natural equivalent, but nothing is shipped yet. |
| Windows | Planned, not yet implemented | The `cursdel-windows` engine is currently a stub. The product design calls for the Windows Restart Manager API as the first-choice mechanism, per the original architecture brief — not yet implemented. |

### Why macOS shells out to `lsof`

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

The Windows engine (`cursdel-windows`) that would actually perform a remote
SMB open closure is not yet implemented — see
[README.md](../README.md#platform-support). On the one platform engine
that *is* implemented today (macOS), `--close-remote-locks` is always
reported unsupported:

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
