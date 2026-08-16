#!/usr/bin/env python3
"""CurseDelete benchmark harness.

Builds synthetic directory trees and times CurseDelete against the
platform's baseline deletion tool(s) (`rm -rf` on Unix, `Remove-Item`/
`rmdir /s /q` on Windows -- Windows comparisons are not implemented by
this script since it has never been run on Windows; see the "Windows"
section of tests/benchmark/README.md for what remains to be done there).

This script does not fabricate numbers: every figure it prints comes from
an actual timed run against a real, freshly-built directory tree on
whatever machine you run it on. It does not claim the results are
representative of any particular storage class (NVMe/SSD/HDD/SMB) unless
you tell it what you ran it against -- see --label.

Usage:
    python3 tests/benchmark/benchmark.py --files 20000 --label "macOS dev sandbox, APFS"

Requires a release build of cursdel on PATH or at --cursdel-bin.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class RunResult:
    label: str
    wall_seconds: float
    peak_rss_bytes: int | None
    files: int
    dirs: int
    exit_code: int


@dataclass
class Scenario:
    name: str
    description: str
    files: int
    dirs: int
    depth: int
    file_size_bytes: int


def build_tree(root: Path, scenario: Scenario) -> None:
    """Build a synthetic tree matching `scenario` under `root`."""
    root.mkdir(parents=True, exist_ok=True)
    payload = b"x" * scenario.file_size_bytes

    if scenario.depth <= 1:
        dirs = [root]
    else:
        dirs = [root]
        current = root
        per_level = max(1, scenario.dirs // max(1, scenario.depth - 1))
        for level in range(scenario.depth - 1):
            next_dirs = []
            for i in range(per_level):
                d = current / f"d{level}_{i}"
                d.mkdir(exist_ok=True)
                next_dirs.append(d)
            dirs.extend(next_dirs)
            current = next_dirs[-1] if next_dirs else current

    files_per_dir = max(1, scenario.files // max(1, len(dirs)))
    remaining = scenario.files
    for d in dirs:
        n = min(files_per_dir, remaining)
        for i in range(n):
            (d / f"f{i}.bin").write_bytes(payload)
        remaining -= n
        if remaining <= 0:
            break
    # Any leftover files (rounding) go in the root.
    i = 0
    while remaining > 0:
        (root / f"extra{i}.bin").write_bytes(payload)
        remaining -= 1
        i += 1


def count_tree(root: Path) -> tuple[int, int]:
    files = 0
    dirs = 0
    for _, dirnames, filenames in os.walk(root):
        dirs += len(dirnames)
        files += len(filenames)
    return files, dirs


def time_command(cmd: list[str], label: str, files: int, dirs: int) -> RunResult:
    """Runs `cmd`, capturing wall time and (on Unix, via GNU/BSD `time -l`
    or `/usr/bin/time -v` where available) peak RSS."""
    system = platform.system()
    peak_rss: int | None = None

    if system == "Darwin":
        # BSD `time -l` prints "maximum resident set size" in bytes on
        # macOS. Wrap the real command so we still capture its own exit
        # code and wall time precisely with time.perf_counter.
        start = time.perf_counter()
        proc = subprocess.run(
            ["/usr/bin/time", "-l"] + cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        elapsed = time.perf_counter() - start
        for line in proc.stderr.splitlines():
            if "maximum resident set size" in line:
                try:
                    peak_rss = int(line.strip().split()[0])
                except ValueError:
                    pass
        exit_code = proc.returncode
    elif system == "Linux":
        start = time.perf_counter()
        proc = subprocess.run(
            ["/usr/bin/time", "-v"] + cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        elapsed = time.perf_counter() - start
        for line in proc.stderr.splitlines():
            if "Maximum resident set size" in line:
                try:
                    peak_rss = int(line.strip().split(":")[1].strip()) * 1024
                except (ValueError, IndexError):
                    pass
        exit_code = proc.returncode
    else:
        start = time.perf_counter()
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        elapsed = time.perf_counter() - start
        exit_code = proc.returncode

    return RunResult(
        label=label,
        wall_seconds=elapsed,
        peak_rss_bytes=peak_rss,
        files=files,
        dirs=dirs,
        exit_code=exit_code,
    )


def find_cursdel(explicit: str | None) -> str:
    if explicit:
        return explicit
    repo_root = Path(__file__).resolve().parents[2]
    release_bin = repo_root / "target" / "release" / "cursdel"
    if release_bin.exists():
        return str(release_bin)
    debug_bin = repo_root / "target" / "debug" / "cursdel"
    if debug_bin.exists():
        print(
            "WARNING: no release build found, benchmarking the DEBUG build. "
            "Results are not representative of real performance; run "
            "`cargo build --release -p cursdel-cli` first for authoritative numbers.",
            file=sys.stderr,
        )
        return str(debug_bin)
    print("error: cursdel binary not found; build it first (cargo build --release -p cursdel-cli)", file=sys.stderr)
    sys.exit(1)


def run_scenario(scenario: Scenario, cursdel_bin: str, workdir: Path) -> list[RunResult]:
    results = []

    for tool_name, cmd_builder in [
        ("cursdel", lambda p: [cursdel_bin, str(p), "--quiet"]),
        ("rm -rf", lambda p: ["rm", "-rf", str(p)]),
    ]:
        target = workdir / f"{scenario.name}_{tool_name.replace(' ', '_')}"
        if target.exists():
            shutil.rmtree(target, ignore_errors=True)
        build_tree(target, scenario)
        files, dirs = count_tree(target)
        result = time_command(cmd_builder(target), tool_name, files, dirs)
        results.append(result)
        if target.exists():
            shutil.rmtree(target, ignore_errors=True)

    return results


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--files", type=int, default=20_000, help="files in the main scenario")
    parser.add_argument("--cursdel-bin", type=str, default=None)
    parser.add_argument("--label", type=str, default="unlabelled run", help="describe the storage/hardware under test")
    parser.add_argument("--out", type=str, default=None, help="write JSON results to this path")
    args = parser.parse_args()

    cursdel_bin = find_cursdel(args.cursdel_bin)

    scenarios = [
        Scenario("flat_small_files", "flat directory of small files", args.files, 1, 1, 256),
        Scenario("wide_dirs", "many directories with a few files each", args.files, max(50, args.files // 20), 2, 512),
        Scenario("deep_tree", "deep narrow tree", max(200, args.files // 50), 100, 50, 1024),
        Scenario("mixed_sizes", "mixed file sizes", args.files, 1, 1, 4096),
    ]

    print(f"CurseDelete benchmark -- {args.label}")
    print(f"Platform: {platform.platform()}")
    print(f"cursdel:  {cursdel_bin}")
    print()

    all_results: dict[str, list[RunResult]] = {}

    with tempfile.TemporaryDirectory(prefix="cursdel-bench-") as tmp:
        workdir = Path(tmp)
        for scenario in scenarios:
            print(f"## {scenario.name} -- {scenario.description}")
            results = run_scenario(scenario, cursdel_bin, workdir)
            all_results[scenario.name] = results
            for r in results:
                rate = r.files / r.wall_seconds if r.wall_seconds > 0 else float("inf")
                rss = f"{r.peak_rss_bytes / (1024*1024):.1f} MB" if r.peak_rss_bytes else "n/a"
                print(
                    f"  {r.label:10s}  {r.wall_seconds:8.3f}s  "
                    f"{rate:10.1f} files/sec  peak_rss={rss}  "
                    f"files={r.files} dirs={r.dirs} exit={r.exit_code}"
                )
            print()

    if args.out:
        serializable = {
            name: [vars(r) for r in results] for name, results in all_results.items()
        }
        Path(args.out).write_text(json.dumps({"label": args.label, "platform": platform.platform(), "results": serializable}, indent=2))
        print(f"Wrote {args.out}")


if __name__ == "__main__":
    main()
