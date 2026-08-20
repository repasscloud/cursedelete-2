# ADR-0005: Symlink/reparse-point safety and the two-layer root check

## Status

Accepted.

## Context

`README.md#appendix-original-productarchitecture-brief` requires two distinct guarantees that are easy to
conflate but must be kept separate:

1. A destructive *target argument* must never be able to resolve to a
   filesystem or share root, including via a symlink/junction that
   disguises one (`C:\DeleteMe\linkToRoot -> C:\`).
2. While enumerating a target's descendants, CurseDelete must never
   *follow* a symlink/junction/reparse point it discovers -- it deletes
   the link object itself. `C:\DeleteMe\junction -> D:\ImportantData` must
   never cause recursion into `D:\ImportantData`.

These sound similar ("don't let a symlink cause a problem") but require
opposite handling: (1) requires *resolving* the symlink to see where it
really points, specifically so a disguised root can be caught; (2)
requires *not resolving* symlinks encountered inside the tree, so the
delete boundary can't silently expand.

## Decision

- **Target validation** (`cursdel_core::target::validate_target`) resolves
  symlinks fully via `std::fs::canonicalize` (with a fallback that
  canonicalises the parent and re-joins the final component, for a
  dangling symlink whose ultimate target does not exist) before running
  the root/share-root check against the resolved path. This is what makes
  `rejects_symlink_that_resolves_to_filesystem_root` pass: a symlink
  pointing at `/` is caught even though the literal argument string is
  not `/`.
- **Enumeration** (`cursdel_core::walk::stream_tree`, and every platform
  `DirLister`) never recurses into a discovered entry whose native
  attributes mark it a reparse point/symlink (`RawChild::is_reparse_point`).
  Such an entry is always emitted as a leaf (`EntryKind::Symlink`) and
  deleted directly -- see `walk::tests::does_not_recurse_into_directory_symlinks`,
  which asserts the fake `DirLister` is never asked to list the target of
  a directory symlink, because it is never pushed onto the walk stack.
- The **actual delete call** for a leaf symlink dispatches to
  `delete_dir` rather than `delete_file` when the platform reports the
  link itself as a directory-type reparse point (`reparse_is_dir`,
  Windows junctions/directory symlinks) -- this removes the link object
  (`RemoveDirectoryW`-equivalent) without ever touching what it points to.
  On POSIX this distinction does not exist (`unlink()` removes any
  symlink uniformly), so `reparse_is_dir` is always `false` there.
- If a platform engine cannot determine with confidence what an object on
  disk actually is (attribute read failure, ambiguous filesystem
  behaviour), the product rule is "fail that object and report it instead
  of guessing" -- engines should surface this as a `DeleteFailure` rather
  than choosing a recursion behaviour speculatively. This is a directive
  for platform engine implementations (Windows/Linux/macOS) rather than
  something `cursdel-core` enforces mechanically, since "ambiguous" is
  necessarily platform-specific.

## Consequences

- Root-escape via a disguised symlink and tree-escape via a discovered
  symlink are both prevented, by different, independently-tested
  mechanisms, rather than one mechanism doing double duty (which risks
  getting exactly one of the two guarantees backwards).
- A target that is *itself* a symlink is a valid, safe delete target: only
  the link is ever removed (`allows_dangling_symlink_as_delete_target`,
  `allows_symlink_that_resolves_to_ordinary_directory`).
- Mount points are treated as ordinary directories by this layer; a target
  crossing a mount boundary is not specially rejected (the product brief
  does not require this), but platform engines should not assume a
  mounted filesystem behaves identically to the host filesystem for
  attribute/permission semantics -- see the per-platform engine docs.
