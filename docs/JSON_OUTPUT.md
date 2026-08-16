# JSON Output (`--json`)

`cursdel --json <target> [options]` replaces the normal text summary with a
single machine-readable JSON object on stdout. This is a public, stable
contract for automation — see [EXIT_CODES.md](EXIT_CODES.md) for the exit
code that accompanies it. The schema and field names below are taken
directly from [`crates/cursdel-core/src/report.rs`](../crates/cursdel-core/src/report.rs)
(`JsonReport`), which is the single source of truth; both must be kept in
sync with any change there.

## Schema versioning

Every report carries `schemaVersion`, currently `1`
(`JSON_SCHEMA_VERSION` in `report.rs`). Field names and types will not
change under a given schema version — a breaking change bumps the version
rather than silently altering the shape. Consumers should check
`schemaVersion` before parsing rather than assuming it will always be `1`.

## Verified example

Captured from a real run against a small on-disk scratch tree
(`cursdel <target> --json --dry-run`):

```json
{
  "schemaVersion": 1,
  "target": "/private/tmp/scratch/Logs",
  "mode": "normal",
  "dryRun": true,
  "workersRequested": "auto",
  "workersFinal": 10,
  "aclRepairEnabled": false,
  "killLocksEnabled": false,
  "remoteLocksEnabled": false,
  "elapsedMs": 213,
  "interrupted": false,
  "metrics": {
    "filesScanned": 2,
    "dirsScanned": 2,
    "filesDeleted": 2,
    "dirsDeleted": 2,
    "filesRetained": 0,
    "dirsRetained": 0,
    "bytesDeleted": 10,
    "failures": 0,
    "remediationAttempts": 0,
    "remediationSuccesses": 0,
    "localLocksResolved": 0,
    "remoteLocksResolved": 0,
    "retries": 0
  },
  "failures": [],
  "failuresTruncated": false,
  "exitCode": 0
}
```

(`target` above is shortened for readability; a real run prints the full
absolute path.)

## Top-level fields

| Field | Type | Meaning |
|---|---|---|
| `schemaVersion` | number | Always `1` for this document's version of the schema. |
| `target` | string | The target path exactly as passed to `cursdel`, rendered via the platform's path display. |
| `mode` | string | `"normal"`, `"force"`, or `"destroy"` — see [COMMAND_REFERENCE.md](COMMAND_REFERENCE.md#mode). |
| `dryRun` | boolean | True if `--dry-run` was set; when true, `metrics` reports what *would* happen and no filesystem changes were made. |
| `workersRequested` | string | `"auto"`, or the exact string of a fixed `--workers N` value. |
| `workersFinal` | number | The worker count actually in effect when the run finished — for `auto`, this is wherever the adaptive controller settled; for a fixed count, it always equals the requested value. |
| `aclRepairEnabled` | boolean | Whether permission/ownership/ACL remediation was active (`--force` or `--destroy`). |
| `killLocksEnabled` | boolean | Whether local lock termination was active (`--kill-locks`, or implied by `--destroy`). |
| `remoteLocksEnabled` | boolean | Whether `--close-remote-locks` was active. |
| `elapsedMs` | number | Wall-clock duration of the operation, in milliseconds. |
| `interrupted` | boolean | True if the run was cancelled (Ctrl+C) before completing; `metrics` then reflects partial progress. |
| `metrics` | object | See below. |
| `failures` | array | Per-object failure details (see below); may be a truncated subset — check `failuresTruncated`. |
| `failuresTruncated` | boolean | True if more failures occurred than are represented in `failures`. The **count** in `metrics.failures` is always exact regardless of this flag; only the per-object detail list is capped (at 10,000 entries) to keep memory bounded during a pathological failure storm. Use `--log` for the full text report if you need every detail line. |
| `exitCode` | number | The same value the process exits with — included so a consumer parsing only the JSON body doesn't also need to check `$?`. See [EXIT_CODES.md](EXIT_CODES.md). |

## `metrics` object

All counters are exact for the *count* fields; `bytesDeleted` reflects only
successfully deleted (or, under `--dry-run`, would-be-deleted) files.

| Field | Meaning |
|---|---|
| `filesScanned` | Total files discovered during enumeration. |
| `dirsScanned` | Total directories discovered (including the target root itself). |
| `filesDeleted` | Files actually deleted (or, under `--dry-run`, that would qualify for deletion). |
| `dirsDeleted` | Directories actually deleted (or would be, under `--dry-run`). |
| `filesRetained` | Files intentionally kept — filtered out by `--age`/`--include`/`--exclude`/`--min-size`/`--max-size`. Not a failure. |
| `dirsRetained` | Directories left behind because they were not empty after their children were processed (typically because some children were retained). Not a failure. See [RETENTION.md](RETENTION.md). |
| `bytesDeleted` | Total size, in bytes, of deleted (or simulated-deleted) files. |
| `failures` | Count of objects that could not be deleted. This drives the `CompletedWithFailures` exit code. |
| `remediationAttempts` | Number of times `--force`/`--destroy` remediation (attribute/ownership/ACL repair) was attempted after an initial delete failure. |
| `remediationSuccesses` | Of those attempts, how many were followed by a successful retry. |
| `localLocksResolved` | Number of objects unlocked via `--kill-locks` and then successfully deleted. |
| `remoteLocksResolved` | Number of objects unlocked via `--close-remote-locks` and then successfully deleted. |
| `retries` | Total retry attempts across remediation and lock resolution combined. |

## `failures[]` entries

Each entry is a [`DeleteFailure`](../crates/cursdel-core/src/error.rs),
serialized with `camelCase` field names:

| Field | Type | Meaning |
|---|---|---|
| `path` | string | The object that could not be deleted. |
| `isDirectory` | boolean | Whether the failed object was a directory. |
| `category` | string | A stable, `snake_case` failure category — see the table below. `--json` consumers should match on this, not on `message`. |
| `message` | string | Human-readable detail (OS error text). Not guaranteed stable across platforms or OS versions — do not parse it. |
| `osErrorCode` | number \| null | The raw OS error code, if one was available, for deeper diagnostics. |

`category` values (from `FailureCategory`, `crates/cursdel-core/src/error.rs`):

`access_denied`, `sharing_violation`, `not_found`, `not_empty`,
`path_too_long`, `reparse_point_refused`, `outside_boundary`,
`remote_lock_unsupported`, `remote_lock_failed`, `local_lock_unresolved`,
`io`, `other`.

`local_lock_unresolved` and the two `remote_lock_*` categories are what
drive the `LockResolutionFailed`/`RemoteLockHandlingFailed` exit codes when
the corresponding flag was enabled — see [EXIT_CODES.md](EXIT_CODES.md).

## Notes for consumers

- `--json` is mutually meaningful with `--dry-run`: combine them to get a
  structured "would delete" plan without touching the filesystem.
- `--json` output goes to stdout as a single pretty-printed JSON document
  (not JSON Lines); pair with `--log <path>` to also persist it to a file.
- `--quiet` does not suppress `--json` output — quiet mode only affects the
  plain-text renderer.
