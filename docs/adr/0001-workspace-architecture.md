# ADR-0001: Workspace and platform architecture

## Status

Accepted.

## Context

`README-CurseDelete2.md` requires shared orchestration with native,
platform-specific deletion engines for Windows, Linux, and macOS, rather
than a lowest-common-denominator abstraction. The previous implementations
inform this decision:

- `_old/CurseDelete` (C#) is fully cross-platform through .NET's `System.IO`
  abstraction. It gets the **pipeline shape** right (a bounded
  `BlockingCollection` producer/consumer queue with workers started before
  enumeration finishes) but has no platform-native code at all -- deletion
  goes through `File.Delete`/`Directory.Delete`, so it cannot use
  `FILE_DISPOSITION_INFO_EX`, Restart Manager, or POSIX `openat`/`unlinkat`.
- `_old/sfvdd` (Rust, Windows-only) gets the **native API usage** right
  (`FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH`,
  `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS`, ownership/ACL remediation via
  `SetNamedSecurityInfoW`) but its "fast" path
  (`force_delete_tree_fast`/`collect_tree_fast`) collects the **entire**
  file and directory list into `Vec`s before deleting anything -- exactly
  the anti-pattern the product brief prohibits ("never require the entire
  target tree to be loaded into memory before deletion starts").

Neither codebase can simply be extended: the C# implementation would need a
parallel native-interop layer bolted on per platform, and `sfvdd`'s
memory-unbounded collection strategy is a correctness/scalability defect
the new engine must not inherit.

## Decision

A Cargo workspace with:

- `cursdel-core`: shared orchestration. Owns target/root-safety validation,
  the streaming enumeration-to-deletion pipeline, the bounded work queue,
  the adaptive worker pool, filters/retention, reporting, and the
  `PlatformEngine` trait every platform engine implements. Contains no
  platform-specific code.
- `cursdel-windows`, `cursdel-linux`, `cursdel-macos`: one crate per
  platform, each implementing `PlatformEngine` with that OS's fastest
  native primitives. Each crate is `cfg`-gated at the crate root
  (`#![cfg(windows)]` etc.) so it compiles to an empty crate on other
  targets rather than being excluded from the workspace graph -- this
  keeps `cargo check --workspace` meaningful on every host while still
  allowing genuinely OS-specific code (Win32 FFI, `libc` calls) inside.
- `cursdel-policy` / `cursdel-license`: licence verification and
  edition/capability policy, decoupled from the deletion engine (see
  ADR-0004).
- `cursdel-cli`: the `cursdel` binary, wiring CLI parsing to
  `cursdel-core::pipeline` and selecting the platform engine via `cfg`.

### Shared tree walker

A further refinement beyond the layout `README-CurseDelete2.md` sketches:
the tricky, easy-to-get-wrong part of enumeration -- explicit-stack
traversal (no recursion, so arbitrarily deep trees can't blow the call
stack), `DirId` allocation, and emitting `DirectoryComplete` events in a
way that is *always* eventually consistent even under cancellation -- is
implemented exactly once, in `cursdel_core::walk::stream_tree`. Platform
engines implement only a `DirLister` (`list_children(dir) ->
Vec<RawChild>`) using their native single-directory listing call
(`FindFirstFileExW`, `readdir`/`getdents64`) and call `stream_tree`. This
was not in the original sketch but is a direct consequence of engineering
rule "keep platform-specific behaviour in platform-specific code" applied
strictly: the traversal algorithm is not platform-specific, only the
syscall that lists one directory is.

## Consequences

- Adding a platform means implementing one trait (`PlatformEngine`) plus,
  in practice, one small `DirLister` -- not re-deriving the whole
  concurrency/streaming/safety design.
- `cursdel-core` is fully testable (and is tested: 70+ unit tests) on any
  development host, including this repository's macOS environment, without
  needing real Windows/Linux hardware for the orchestration logic.
- Platform crates can still only be *fully* validated (build + run) on
  their real OS; cross-target `cargo check` (this repo installs
  `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu` toolchains for exactly this purpose) catches
  type errors and API misuse but cannot exercise real filesystem behaviour.
  CI runs the real test suite on Windows, Linux, and macOS runners (see
  `.github/workflows/`).
