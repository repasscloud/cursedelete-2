# CurseDelete benchmark harness

`benchmark.py` builds synthetic directory trees and times CurseDelete
against the platform baseline deletion tool(s), capturing wall time,
files/sec, and (on macOS/Linux) peak RSS via `/usr/bin/time`.

**No numbers in this repository's documentation are fabricated.** Every
figure attributed to a run of this harness was produced by actually
running it; see `docs/BENCHMARKS.md` for the results captured during
CurseDelete 2's development and their explicit hardware/environment
caveats.

## Running it

```bash
cargo build --release -p cursdel-cli
python3 tests/benchmark/benchmark.py --files 20000 --label "describe your hardware here"
```

`--files` controls scenario scale; `--label` should describe the actual
storage/hardware under test (e.g. "Windows Server 2022, local NVMe" or
"macOS, SMB share over 1 Gbps LAN to Synology NAS") since the *whole
point* of the adaptive worker model (`docs/adr/0003-adaptive-workers.md`)
is that the right concurrency differs by target class -- a benchmark
result is meaningless without saying what it was run against.

Add `--out results.json` to also write a machine-readable copy.

## Scenarios

- `flat_small_files` -- one flat directory of small files.
- `wide_dirs` -- many directories, a few files each.
- `deep_tree` -- a deep, narrow tree.
- `mixed_sizes` -- a flat directory of slightly larger files.

These are intentionally modest defaults (thousands, not millions, of
files) so the harness runs in a reasonable time in a typical development
or CI environment. **They are not a substitute for the large-scale runs
(1,000,000+ files, real NVMe/SSD/HDD, real SMB at both LAN and WAN
latency) the product brief calls for** -- those require dedicated
hardware this development environment does not have, and running them
here would not produce numbers worth trusting anyway (a shared/sandboxed
dev container's storage and CPU characteristics are not representative of
a real deployment target). To run an authoritative benchmark:

1. Provision the target hardware/environment you actually care about
   (local NVMe, local HDD, SMB over LAN, SMB over a higher-latency WAN
   link, a real NAS, etc).
2. `cargo build --release -p cursdel-cli` on that machine.
3. `python3 tests/benchmark/benchmark.py --files 1000000 --label "<real description>"`.
4. On Windows, also compare against `Remove-Item -Recurse -Force`,
   `cmd /c rmdir /s /q`, and (if you still have them) the previous
   CurseDelete (C#) and `sfvdd` binaries from `_old/` -- this script does
   not currently drive those (see "What's not implemented" below).
5. Record the results (and the exact hardware/network description) in
   `docs/BENCHMARKS.md` or a dated results file alongside it.

## What's not implemented yet

- **Windows comparisons** (`Remove-Item`, `robocopy` empty-tree trick,
  `cmd /c rmdir /s /q`, the previous C# CurseDelete, `sfvdd`): this
  harness has only run on macOS so far (this repository's development
  environment). The Python script structure (`run_scenario`,
  `time_command`) is written to be extended with a `platform.System() ==
  "Windows"` branch adding those comparison commands and a
  `Get-Process`-based or `/proc`-equivalent memory sampling method: do
  this on a real Windows machine, since it can't be authored blind.
- **Linux `find -delete` comparison**: straightforward to add
  (`["find", str(path), "-delete"]`) but not yet wired in since it hasn't
  been run/validated on a real Linux machine either.
- **Locked-file and permission-heavy scenarios**: the product brief calls
  for benchmarking against read-only files, restrictive ACLs, and locked
  files. These need per-platform setup (creating real ACL-restricted
  files on Windows, real lock holders) that's more involved than the
  synthetic trees this script builds; left as a documented follow-up
  rather than a rushed, unrealistic simulation.
- **SMB**: needs a real file server, which this environment does not
  have. See `docs/adr/0006-posix-toctou.md` and
  `docs/adr/0007-windows-engine.md` for what's already implemented for
  remote paths even without a live server to benchmark against.

## Metrics captured

- Wall-clock time (`time.perf_counter`, wrapping the whole subprocess).
- Derived files/sec.
- Peak RSS (`/usr/bin/time -l` on macOS, `/usr/bin/time -v` on Linux; not
  captured on Windows by this script yet).

Not yet captured by this harness (all called for by the product brief):
directories/sec, p95/p99 delete latency, retry count, remediation count,
worker count over time. These require `cursdel --json`'s summary to grow
additional fields (it currently reports totals, not a time series) or a
`--verbose`-level structured event stream; tracked as follow-up work, not
implemented here to avoid adding output-format surface area that hasn't
been thought through as carefully as the rest of the JSON schema
(`docs/JSON_OUTPUT.md`).
