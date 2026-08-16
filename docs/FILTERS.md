# Filters

`--include`, `--exclude`, `--min-size`, and `--max-size` are composable
file filters, layered on top of (and combinable with) `--age`/`--age-by`
retention filtering (see [RETENTION.md](RETENTION.md)). This document
covers glob syntax, evaluation precedence, and the size unit convention.
The filter model is deliberately small — CurseDelete is not a general
filesystem query engine, and the filter set is not intended to grow much
beyond what's documented here.

## What filters apply to

All filters apply to **files only**. Directories are never matched
directly by `--include`, `--exclude`, `--min-size`, or `--max-size` — a
directory is removed once it becomes empty after its qualifying children
are processed, and retained otherwise. See
[RETENTION.md#directory-behavior](RETENTION.md#directory-behavior) for the
full mechanics, and
[SAFETY_MODEL.md](SAFETY_MODEL.md#retention-modes-directory-root-preservation-rule)
for why the target directory itself is never removed once any filter is in
effect.

## `--include <GLOB>`

Only delete files whose **base name** (not the full path) matches this glob
pattern.

```bash
cursdel /var/cache/build --include "*.log"
```

A file that doesn't match `--include` is retained, not an error.

## `--exclude <GLOB>`

Never delete files whose base name matches this glob pattern.

```bash
cursdel /var/cache/build --exclude "*.keep"
```

### Precedence: exclude always wins

When both `--include` and `--exclude` are given and a file matches both,
**exclude wins**:

```bash
cursdel Logs --include "*.log" --exclude "*.keep.log"
# a.keep.log matches *.log (included) AND *.keep.log (excluded) -> retained
```

This is checked first in evaluation order (`exclude`, then `include`, then
size, then age — see `FilterSet::evaluate_file` in
[`crates/cursdel-core/src/filter.rs`](../crates/cursdel-core/src/filter.rs)),
so there is no ambiguity about which rule governs when patterns overlap:
exclusion is always the more specific, more conservative instruction and it
always takes priority.

## Glob syntax

Patterns use standard shell-style globs (via the `globset` crate):
`*` matches any sequence of characters, `?` matches a single character,
`[...]` matches a character class, and `{a,b}` alternation is supported.
Patterns are matched against the file's base name only — there is no
recursive `**` path-matching, because filters intentionally operate on
"which files," not "which subtrees." If you need to scope a filter to a
subtree, point `TARGET` at that subtree directly rather than trying to
encode a path pattern.

## `--min-size <SIZE>` / `--max-size <SIZE>`

Only delete files whose size falls within the given bound(s). Both can be
combined for a range.

```bash
cursdel /var/cache/build --min-size 10m
cursdel D:\BuildCache --min-size 10m --max-size 5g
```

A bare number (no suffix) means bytes — unlike `--age`, there's no
ambiguity to guard against here, since a byte count has no competing
interpretation.

### Size unit convention

Suffixes are **binary (1024-based)**, matching `du`, `ls -h`, and most
sysadmin tooling, not SI/decimal (1000-based) units:

| Suffix | Also accepts | Multiplier |
|---|---|---:|
| `k` | `kb`, `kib` | 1024 |
| `m` | `mb`, `mib` | 1024² |
| `g` | `gb`, `gib` | 1024³ |
| `t` | `tb`, `tib` | 1024⁴ |

So `--min-size 100m` means 100 × 1024² = 104,857,600 bytes, **not**
100,000,000 bytes. This is a deliberate, documented choice
([`crates/cursdel-core/src/size.rs`](../crates/cursdel-core/src/size.rs)),
not an accidental SI/IEC mixup: the "b/m/g/t" suffix family is treated as
shorthand for the same binary magnitudes `k`/`m`/`g`/`t` already mean in
`du -h`, `ls -lh`, and equivalent tools that most operators reach for this
tool alongside — using SI multipliers under the same short suffixes would
be the surprising choice for that audience, not the safe one. If you need
to be unambiguous either in scripts or in your own head, the `ki`/`mi`/
`gi`/`ti`-style spellings (`kib`, `mib`, `gib`, `tib`) are accepted too and
carry the same binary values.

Values are rounded to the nearest byte after multiplication, so fractional
suffixed values (`--min-size 1.5g`) work as expected.

## Combining filters

All filters compose — retention, include/exclude, and size bounds are all
independently optional and all must pass for a file to qualify for
deletion:

```bash
cursdel \\FS01\Logs --age 90d --include "*.log"
cursdel D:\BuildCache --age 14d --min-size 10m
```

Combine with `--dry-run` (see [QUICKSTART.md](QUICKSTART.md)) before
running any filtered operation for real, especially the first time you use
a new pattern.
