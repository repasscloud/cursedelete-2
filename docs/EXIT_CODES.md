# Exit Codes

`cursdel` returns a stable, documented process exit code on every run. The
codes are part of CurseDelete's automation contract: scripts and CI systems
depend on the numeric values, not just what the tool printed. Once released,
a code's meaning is frozen — new situations get new codes rather than a code
being repurposed. The authoritative source is
`crates/cursdel-core/src/exit_code.rs`.

| Code | Name | Meaning |
|---:|---|---|
| `0` | `Success` | The operation completed with zero failures. |
| `1` | `InvalidArgsOrTarget` | The target was rejected by safety validation (filesystem root, share root, resolves-to-root via `..`/symlink — see [SAFETY_MODEL.md](SAFETY_MODEL.md)), or another invalid-argument condition caught before the operation started. |
| `2` | `CompletedWithFailures` | The operation ran to completion but one or more objects could not be deleted. This does **not** include files that were merely filtered or retained by `--age`/`--include`/`--exclude`/`--min-size`/`--max-size` — that is expected, successful behavior, not a failure. |
| `3` | `PrivilegeRequirementNotSatisfied` | A required permission or privilege was not satisfied — for example, `--force` was requested but the executing security context lacks the authority needed to remediate. |
| `4` | `LockResolutionFailed` | `--kill-locks` was requested and failed to free one or more locked objects. |
| `5` | `RemoteLockHandlingFailed` | `--close-remote-locks` was requested and failed. |
| `6` | `Interrupted` | The operation was interrupted (Ctrl+C) before it could finish. Partial results are still reported — see the `interrupted` field in [JSON_OUTPUT.md](JSON_OUTPUT.md). |
| `7` | `LicenseRequired` | A capability that requires a valid license entitlement was requested without one — currently only `--close-remote-locks` (see [LICENSING.md](LICENSING.md)) and license activation/import/refresh failures. |
| `64` | `CliUsageError` | A generic CLI usage error: bad flag value, missing required argument, unparseable `--age`/`--workers`/size value, invalid glob pattern. This is the conventional BSD `EX_USAGE` value. |
| `99` | `UnexpectedFatal` | An unexpected fatal error not covered by a more specific code (e.g. the license file could not be written to disk). |

## Precedence when multiple conditions apply

A single run can only return one exit code, so [`Summary::exit_code`](../crates/cursdel-core/src/report.rs)
applies a fixed precedence, checked in this order:

1. **`Interrupted` (6) wins over everything else.** If the run was
   cancelled, that is reported regardless of what else happened — even if
   failures were also recorded before the interrupt.
2. **`RemoteLockHandlingFailed` (5)**, if `--close-remote-locks` was enabled
   and at least one failure is categorized as a remote-lock failure.
3. **`LockResolutionFailed` (4)**, if `--kill-locks` (or `--destroy`, which
   implies it) was enabled and at least one failure is categorized as an
   unresolved local lock.
4. **`CompletedWithFailures` (2)**, for any other non-empty failure list.
5. **`Success` (0)**, otherwise.

`InvalidArgsOrTarget` (1), `CliUsageError` (64), `LicenseRequired` (7), and
`UnexpectedFatal` (99) are returned directly by `cursdel-cli` before an
operation summary even exists — they short-circuit the run rather than
appearing in this precedence chain.

## Verified examples

```console
$ cursdel /
Error: refusing to delete filesystem root '/': CurseDelete never deletes a
drive, volume, or filesystem root. Pass a path at least one level below the
root.
$ echo $?
1

$ cursdel
Error: a target path is required.

Usage: cursdel <path> [options]
Run 'cursdel --help' for details.
$ echo $?
64

$ cursdel /tmp --age 2
Error: '--age 2' is missing a unit. A duration unit is mandatory: use m
(minutes), h (hours), d (days), or w (weeks), e.g. --age 2d
$ echo $?
64

$ cursdel /tmp --close-remote-locks
Error: --close-remote-locks requires a Business or Enterprise licence.
Run 'cursdel license status' for details, or 'cursdel license activate' to
activate one.
$ echo $?
7
```

Each example above was captured from a real run of the built `cursdel`
binary, not transcribed by hand.

## Automation guidance

- Treat `0` as the only "fully clean" result. Anything else means either the
  invocation itself was wrong (1, 64, 7) or the operation ran but left work
  undone (2, 3, 4, 5, 6, 99).
- `2` is common and expected for large, unattended retention jobs against
  live filesystems (a handful of objects busy at scan time is normal); do
  not treat it identically to `1` in alerting.
- For scripted retries, only `4`/`5` (lock-related) and `6` (interrupted)
  are generally safe to retry as-is; `1`, `3`, `7`, and `64` need the
  invocation itself corrected first.
