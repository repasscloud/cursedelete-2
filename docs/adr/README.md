# Architecture Decision Records

These records document the decisions made while implementing CurseDelete 2,
including places where the implementation refines or deviates from
`README.md#appendix-original-productarchitecture-brief` (the original product/architecture brief) and why.
Read them when you need the reasoning behind a design choice, not just the
choice itself — the user-facing docs in `docs/` link back to specific ADRs
wherever a decision needs justification rather than restatement.

| ADR | Title | Summary |
|---|---|---|
| [0001](0001-workspace-architecture.md) | Workspace and platform architecture | Why the workspace is one shared `cursdel-core` crate plus one crate per platform (`cursdel-windows`/`cursdel-linux`/`cursdel-macos`), each implementing a single `PlatformEngine` trait, with the tree-walking algorithm itself implemented exactly once in `cursdel-core::walk` rather than duplicated per platform. |
| [0002](0002-streaming-pipeline.md) | Streaming enumeration/deletion pipeline | The bounded-channel pipeline that lets deletion start before enumeration finishes, with structural (not bolted-on) backpressure, separate worker pools for files vs. directories, and cooperative Ctrl+C cancellation. |
| [0003](0003-adaptive-workers.md) | Adaptive worker model | Pre-spawned, semaphore-gated worker threads plus a pure hill-climbing controller that grows/shrinks concurrency from observed throughput and error rate, including why probe direction must alternate to avoid a long-run upward drift. |
| [0004](0004-licensing-integration.md) | Licensing integration architecture | Why the license verifier is a from-scratch, byte-exact Rust reimplementation of the .NET `Licensing.Core` signing scheme rather than an FFI bridge or sidecar process, how it was validated against real C#-signed fixtures, and why credentials are stored in a protected file rather than an OS keychain. |
| [0005](0005-symlink-reparse-safety.md) | Symlink/reparse-point safety and the two-layer root check | The two independent guarantees that are easy to conflate — resolving symlinks to catch a disguised root at the target argument, versus never resolving symlinks discovered inside the tree during enumeration — and why they need opposite handling. |
| [0006](0006-posix-toctou.md) | TOCTOU mitigation on POSIX delete operations | The `openat`/`unlinkat`/`fstatat`-relative delete pattern that closes the final-component race, and the honestly-documented residual risk it does *not* close (an ancestor directory replaced mid-walk, several levels up). |
| [0007](0007-windows-engine.md) | Windows native engine | `FILE_DISPOSITION_INFO_EX` as the primary delete path with a classic `DeleteFileW`/`RemoveDirectoryW` fallback (always reporting the fallback's own result, never discarding it), unconditional `FILE_FLAG_OPEN_REPARSE_POINT` so a reparse point is never followed, ownership/ACL remediation scoped to the current token's actual authority, Restart Manager plus direct `TerminateProcess` for `--kill-locks`, and `NetFileEnum`/`NetFileClose` for `--close-remote-locks`. |
| [0008](0008-linux-engine.md) | Linux native engine | Why creation time uses `statx` with a `fstatat`/`None` fallback rather than always reporting `None`, why `opendir`/`readdir` ships instead of raw `getdents64`, and why `--kill-locks` lock-holder discovery reads `/proc/[pid]/fd/*` natively instead of shelling out to `lsof` the way the macOS engine does. |
| [0009](0009-source-license-open-question.md) | Source code license is an open question | Flags a real metadata inconsistency found and fixed (`Cargo.toml` claimed a commercial license with no corresponding text; the actual checked-in `LICENSE` is a generic Apache-2.0 bootstrap default) and why choosing the product's real source license is a legal/business decision this implementation does not make unilaterally. |

## Adding a new ADR

Number sequentially, use the `NNNN-short-title.md` filename pattern already
in use, and add a row to the table above. An ADR records a decision and its
reasoning at the time it was made — update the linked document (in `docs/`)
when user-facing behavior changes, but only revise or supersede the ADR
itself if the underlying decision changes.
