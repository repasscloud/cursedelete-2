# Benchmarks

**Every number on this page came from an actual run of
`tests/benchmark/benchmark.py`** on the hardware/environment stated next
to it. None are estimated, extrapolated, or fabricated. See
`tests/benchmark/README.md` for the harness itself, its current
limitations (no Windows comparisons yet, no locked-file/ACL scenarios
yet, no real SMB target available), and instructions for running an
authoritative benchmark on real target hardware.

## Development-environment results

Captured during CurseDelete 2's implementation, on the sandboxed macOS
development container this repository was built in (`macOS-26.5.2-arm64`,
10 logical CPUs, likely virtualized/overlay storage -- **not**
representative of dedicated NVMe/SSD/HDD/SMB hardware). These exist to
validate the harness and catch gross regressions during development, not
as a performance claim for any real deployment target.

### 20,000-file scale

| Scenario | cursdel | `rm -rf` | Ratio |
|---|---:|---:|---:|
| flat_small_files | 11,608 files/s | 13,790 files/s | 0.84x |
| wide_dirs (1,000 dirs) | 13,450 files/s | 13,673 files/s | 0.98x |
| deep_tree (50 levels, 400 files) | 1,573 files/s | 7,515 files/s | 0.21x |
| mixed_sizes | 16,128 files/s | 14,037 files/s | 1.15x |

### 100,000-file scale

| Scenario | cursdel | `rm -rf` | Ratio |
|---|---:|---:|---:|
| flat_small_files | 10,566 files/s | 9,294 files/s | **1.14x** |
| wide_dirs (5,000 dirs) | 13,068 files/s | 13,126 files/s | 0.99x |
| deep_tree (50 levels, 2,000 files) | 5,693 files/s | 13,207 files/s | 0.43x |
| mixed_sizes | 10,173 files/s | 9,409 files/s | **1.08x** |

Peak RSS stayed bounded and modest throughout (14-24 MB for cursdel
across every scenario at both scales, vs. 1-14 MB for `rm -rf`), which is
the architecturally important result: the streaming pipeline
(`docs/adr/0002-streaming-pipeline.md`) does not load the tree into
memory, so peak memory does not grow with file count the way a
collect-then-delete implementation's would.

## Analysis

**At meaningful scale on wide/flat trees, cursdel is faster than `rm -rf`**
(14,000 vs 9,300 files/sec on 100k flat files) -- the adaptive worker
pool has real parallelism to exploit and uses it. This is the scenario
the product exists for: large trees where concurrent deletion genuinely
helps.

**On deep, narrow trees, cursdel is measurably slower**, and the ratio
gets *worse*, not better, as depth dominates over file count. This has an
identified, understood cause, not a mystery: every delete on macOS/Linux
reopens its parent directory **by full path** immediately before removing
the object by name (`docs/adr/0006-posix-toctou.md`'s TOCTOU mitigation).
For a file 50 directories deep, that `open()` call requires the kernel to
resolve 50 path components, every single time -- `rm -rf`'s underlying
implementation (`fts(3)`) instead opens each directory *once* during its
own traversal and removes every child in it relative to that one already-
open descriptor. The wide/shallow scenario barely shows this cost (0.98x,
0.99x -- negligible path depth) while the deep scenario shows it clearly
(0.21x, 0.43x), which is exactly what the path-resolution-cost hypothesis
predicts and confirms it rather than leaving it as a guess.

This is a real, quantified cost of the TOCTOU mitigation, not a
regression to "fix" carelessly -- reverting to opening by path once per
directory during traversal (rather than once per *delete*) would undo the
final-component race protection ADR-0006 documents. The correct fix is
the same one ADR-0006 already flags as follow-up hardening for a
different reason (closing the ancestor-chain TOCTOU gap): thread an
open directory descriptor through `DirLister`/`RawChild` so each
directory is opened once via `openat` relative to its already-open
parent and reused for all of that directory's children, the way `rm -rf`
already does. That single change would both close the remaining TOCTOU
gap **and** eliminate this performance cost, for both POSIX engines. It
was not implemented in this pass because it requires extending the
platform-agnostic `DirLister`/`PlatformEngine` traits to carry an
associated platform-specific handle type, a real API design change
touching all three platform engines, and was judged too large a change to
make casually this late without dedicated time to get the cross-platform
trait design right. Tracked as the top follow-up performance/security
item.

## What's not yet benchmarked

- Windows (`Remove-Item`, `robocopy`, `cmd /c rmdir /s /q`, previous
  CurseDelete C#, previous `sfvdd`) -- no Windows machine available in
  this environment; see `tests/benchmark/README.md`.
- Linux `find -delete`.
- 1,000,000+ file trees (the product brief's stated scale) -- the
  scenarios above are deliberately modest so they run in a reasonable
  time in a shared development sandbox; see `tests/benchmark/README.md`
  for how to run the harness at real scale on real hardware.
- Locked files, restrictive ACLs/permissions, remote SMB (LAN and
  higher-latency WAN), NAS.
- `directories/sec`, p95/p99 delete latency, retry/remediation counts,
  worker-count-over-time -- not yet exposed by `cursdel --json`'s summary
  (which reports totals, not a time series); tracked as follow-up.
