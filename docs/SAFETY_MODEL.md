# Safety Model

CurseDelete is intentionally destructive, so its safety guarantees are as
important as its speed. This document covers four independent mechanisms:
root/share-root protection, symlink/reparse-point handling, the POSIX
TOCTOU (time-of-check-to-time-of-use) mitigation and its honest residual
risk, and retention mode's directory root-preservation rule. Each is
implemented and tested independently — they answer different questions and
are deliberately not merged into one "be careful with paths" mechanism.

## Root and share-root protection

CurseDelete must never be able to delete a filesystem root or an SMB/UNC
share root, and a path must not be able to disguise itself as one via `..`
traversal, a symlink, or a junction. This is enforced by
[`cursdel_core::target::validate_target`](../crates/cursdel-core/src/target.rs),
the single choke point every destructive target passes through, using two
independent layers.

### Layer 1: pure lexical rejection

A string-only parser (`classify`) understands Windows drive paths
(`C:\...`), Windows UNC/share paths (`\\server\share\...`, including the
`\\?\` and `\\?\UNC\` verbatim forms), and POSIX absolute paths (`/...`)
without touching the filesystem, and resolves `.`/`..` components the same
way a real filesystem would. This exists partly so the full Windows/UNC
safety matrix can be exercised in unit tests on any development host
(including macOS/Linux CI, which cannot literally have a `C:\` drive), and
partly because it alone is sufficient to catch a `..` collapse before any
filesystem call is made.

**Rejected** (from `target.rs`'s own test suite):

```text
/                              (POSIX root)
C:\                            (Windows drive root, with separator)
C:                             (Windows drive root, without separator)
\\server\share\                (UNC share root, with trailing separator)
\\server\share                 (UNC share root, without trailing separator)
\\?\C:\                        (verbatim drive root)
\\?\UNC\server\share            (verbatim UNC share root)
C:\folder\..                   (collapses to C:\)
/var/..                        (collapses to /)
\\server\share\folder\..       (collapses to \\server\share)
```

**Allowed** (also from the same suite — note the last two: `..` is not
rejected wholesale, only when it collapses all the way to a root):

```text
/var/tmp/cache
C:\folder
C:\folder\file.dat
\\server\share\folder
\\server\share\folder\file.dat
C:\folder\sub\..                (resolves to C:\folder — one level below root, fine)
/var/tmp/..                     (resolves to /var — one level below root, fine)
```

Mixed separators (`C:/folder/..`) and both verbatim-prefix spellings are
handled the same way as their canonical forms.

### Layer 2: filesystem canonicalization

The lexical layer alone cannot catch a symlink or junction that *disguises*
a root — e.g. a directory entry `linkToRoot` inside your target tree whose
real target is `/`. Layer 2 resolves the requested path fully via the OS's
own canonicalization (`std::fs::canonicalize`, with a fallback that
canonicalizes the parent and rejoins the final component for a dangling
symlink whose target doesn't exist) and re-runs the same root check against
the *resolved* path.

```text
$ ln -s / /tmp/root_link
$ cursdel /tmp/root_link
Error: target '/tmp/root_link' resolves to protected root '/' after
canonicalisation (e.g. via '..' components); refusing to proceed
```

A symlink that resolves to an ordinary, non-root directory is a perfectly
valid target — only the link itself is what gets deleted, never anything
inside its target. The same is true for a dangling symlink (one whose
target doesn't exist): it's still a valid, safe delete target, because
deleting it only removes the link object.

These two layers ask genuinely different questions — "does the literal
argument denote a root" vs. "does the argument *resolve* to a root" — and
are kept separate deliberately. See
[ADR-0005](adr/0005-symlink-reparse-safety.md) for the full reasoning,
including why this is distinct from the next guarantee below.

## Symlinks and reparse points encountered during enumeration

The guarantee above protects the *target argument*. A separate, narrower
guarantee protects the *walk*: while enumerating a target's descendants,
CurseDelete never follows a symlink, junction, or reparse point it
discovers partway through the tree.

```text
DeleteMe/
└── build-output -> /var/important-data     (a symlink discovered inside the tree)
```

Deleting `DeleteMe` never recurses into `/var/important-data`. The
platform enumerator reports such an entry directly as a leaf
(`EntryKind::Symlink`), and the shared tree walker
(`cursdel_core::walk::stream_tree`) never pushes it onto the traversal
stack — it is deleted as the link object itself (the equivalent of
`unlink()`/`RemoveDirectoryW` on the link, not on whatever it points to),
and whatever it points to is left completely untouched.

On Windows, a directory-type reparse point (a junction or a directory
symlink) is removed via the directory-style native call rather than the
file-style one, because removing a link object still requires matching its
presented type — but this still only removes the link, never recurses
through it. On POSIX this distinction doesn't exist: `unlink()` handles
every symlink type uniformly.

If a platform engine can't determine with confidence what an object on
disk actually is (an attribute read failure, ambiguous filesystem
behavior), the rule is to fail and report that object rather than guess at
a recursion decision — see [ADR-0005](adr/0005-symlink-reparse-safety.md).

Mount points are treated as ordinary directories by this layer; crossing a
mount boundary during a delete is not itself rejected (the product design
does not require it), but be aware a mounted filesystem may not share the
host filesystem's attribute/permission semantics.

## POSIX TOCTOU mitigation (and its honest limit)

A classic `rm -rf` race: list a directory, then later call a delete
function on the full path string again. Between the listing and the
delete, a local attacker with write access to an ancestor directory could
replace an entry with a symlink, redirecting the delete somewhere the
operator never intended. CurseDelete's macOS (and, when implemented,
Linux) engine closes the most dangerous part of this window:

1. **Re-open the target's immediate parent directory** with
   `O_DIRECTORY | O_NOFOLLOW`, obtaining a fresh file descriptor.
   `O_NOFOLLOW` means that if the parent path's final component has been
   swapped for a symlink since it was last validated, the open fails with
   `ELOOP` instead of silently following it.
2. **Call `unlinkat(parent_fd, name, flags)`** (or `fstatat`, for metadata)
   by name, relative to that already-open descriptor — never by
   re-resolving the full path string. The kernel performs the removal
   atomically relative to that specific directory, so there is no window
   between "identify the parent" and "remove the child" where the parent
   itself could be swapped out from under the call.
3. **Directory listing** uses the same `open(O_NOFOLLOW) → fdopendir →
   readdir` pattern, and reads metadata via `fstatat(dir_fd, name,
   AT_SYMLINK_NOFOLLOW)` relative to that same descriptor rather than a
   second path-based `lstat` call.

### What this does and does not close

This closes the **final-component race**: the specific parent directory
named in a delete call cannot have been swapped for something else between
being opened and the `unlinkat` call that follows.

**It does not close the ancestor-chain race.** The file descriptor for the
immediate parent is obtained by re-resolving its path from the walk's
cached `PathBuf` — so an *earlier* ancestor, several levels up and already
visited earlier in the walk, could theoretically be replaced with a
symlink between when the walk first visited it and when a delete deep
inside it finally runs. Closing this fully would require never
re-resolving any path from the root at all — keeping every ancestor
directory's descriptor open for the lifetime of the walk and chaining
`openat` calls relative to each parent's already-open descriptor all the
way down. `cursdel-core`'s shared tree walker is intentionally
platform-agnostic and operates on `PathBuf`, not OS file descriptors or
handles (a fd-based walk doesn't translate to Windows' handle model in the
same shape), so this deeper hardening is tracked as documented follow-up
work rather than implemented in the current pass — see
[ADR-0006](adr/0006-posix-toctou.md) for the full reasoning.

**Accepted residual risk:** a sustained local attacker who can predict
CurseDelete's traversal timing and already has write access to an ancestor
directory *within the target tree* could redirect a deeply nested delete.
This requires local code execution with write access inside the tree being
deleted — a materially narrower threat model than a remote or
unauthenticated attacker, and CurseDelete is an administrator-invoked tool
typically run against trees the operator already controls. This is
strictly stronger than both prior CurseDelete implementations, which had no
TOCTOU mitigation at all — but it is not a claim that TOCTOU races are
fully eliminated, and this document will not pretend otherwise.

Windows has an analogous mechanism (`NtCreateFile` with
`OBJECT_ATTRIBUTES.RootDirectory` set to a parent handle) that the Windows
engine does not currently use either; its own residual risk in this area
will be documented in that engine's ADR once implemented.

## Retention mode's directory root-preservation rule

Plain, unfiltered deletion removes the target itself:

```console
$ cursdel /tmp/scratch/Simple
...
Files:          2
Directories:    2
...
$ ls /tmp/scratch/Simple
ls: /tmp/scratch/Simple: No such file or directory
```

But a filtered or retention-based operation (`--age`, `--age-by`,
`--include`, `--exclude`, `--min-size`, `--max-size`) never deletes the
target directory itself, no matter how empty it ends up:

```console
$ cursdel Logs --age 2d
...
$ ls Logs
Current  yesterday.log
```

`Logs/` survives even though everything that qualified for deletion inside
it is gone. This is
[`OperationOptions::preserve_root`](../crates/cursdel-core/src/options.rs):
the root is preserved exactly when the filter set is not a no-op. The
reasoning: `cursdel Logs` is an instruction to remove `Logs` and everything
in it; `cursdel Logs --age 2d` is a retention *policy* applied inside
`Logs` — the operator's own directory structure (and the directory named on
the command line) should never disappear as a side effect of a cleanup
policy quietly deleting every file it manages. See
[RETENTION.md](RETENTION.md#directory-behavior) for the full worked
example, including why `Ancient/` and `Empty/` disappear in that scenario
while `Logs/` itself does not.

## Verified example: the two locations agree

The dry-run and real-run examples above were captured from the actual
built binary, not transcribed by hand, and the same rule shows up
identically in `--json` output's `dirsRetained`/`dirsDeleted` fields (see
[JSON_OUTPUT.md](JSON_OUTPUT.md)) and in the unit tests in `target.rs` and
`options.rs` referenced throughout this document.
