# CurseDelete

> **Blazing-fast, native, cross-platform deletion for files and directory trees that refuse to die.**

**Product name:** CurseDelete  
**CLI binary:** `cursdel` (`cursdel.exe` on Windows)  
**Implementation:** Rust  
**Target platforms:** Windows, Linux, macOS  
**Status:** Product / architecture specification for CurseDelete 2 rewrite

---

## 1. Product Summary

CurseDelete is a native systems utility designed to delete files and directory trees as quickly and reliably as the underlying operating system, filesystem, storage device, or remote file server will allow.

It is not intended to be a thin wrapper around `rm`, `Remove-Item`, `rmdir`, or a standard library recursive-delete method.

The core design goals are:

- begin deleting immediately while the target tree is still being enumerated;
- process independent files concurrently through a bounded worker pool;
- automatically tune concurrency for the storage target;
- provide an explicit manual worker override;
- use native filesystem APIs on each supported operating system;
- deal aggressively with removable permission, ACL, ownership, and attribute barriers;
- handle local and network-backed filesystem paths exposed by the host operating system;
- optionally identify and terminate local processes that hold blocking file handles;
- on supported Windows file servers, optionally close remote SMB file opens when authorised;
- safely handle symlinks, junctions, mount points, reparse points, and other filesystem boundaries;
- support retention cleanup such as deleting only files older than a specified age;
- never allow deletion of a filesystem root or SMB share root;
- remain predictable enough for unattended automation and business use.

CurseDelete should behave like a serious infrastructure utility even though the brand is intentionally aggressive.

---

## 2. Product Philosophy

The product should answer one question:

> **How quickly can this requested data be made to cease to exist without escaping the requested deletion boundary?**

The fast path must stay fast.

Normal files should not be subjected to expensive ACL inspection, ownership changes, lock investigation, or other remediation work unless the normal deletion operation fails.

The fundamental pipeline is:

```text
target
  |
  v
validate/canonicalise
  |
  v
native enumeration
  |
  +-----------------------------+
  |                             |
  v                             v
bounded file queue        directory tracking
  |                             |
  v                             |
parallel delete workers         |
  |                             |
  +--> success ---------------->|
  |
  +--> permission failure
  |       |
  |       v
  |   remediation queue
  |       |
  |       v
  |   ownership / ACL / attributes
  |       |
  |       v
  |      retry
  |
  +--> sharing/lock failure
          |
          v
      lock resolver
          |
          +--> optional local process termination
          |
          +--> optional supported remote SMB close
          |
          v
         retry

directories are deleted only after their retained/deleted children are known
```

Enumeration and deletion must overlap.

The implementation must never require the entire tree to be loaded into memory before deletion starts.

---

## 3. Why Rust

CurseDelete 2 should be a rewrite in Rust.

Rust is chosen because the product is fundamentally systems software:

- direct Win32 API integration on Windows;
- direct POSIX/Darwin/Linux filesystem operations on Unix-like systems;
- large-scale concurrent filesystem work;
- strict control over allocation and memory use;
- high-performance native binaries;
- no runtime installation requirement;
- strong memory-safety guarantees around a highly destructive and concurrent workload.

C++ can access the same native APIs and is also technically suitable, but it does not provide a filesystem deletion primitive that Rust cannot call.

The objective is not "rewrite it in Rust because Rust is faster than C#."

The objective is:

> **Use native OS primitives with an architecture optimised specifically for deletion.**

---

# 4. Platform Model

CurseDelete should share orchestration and product behaviour, but not force every platform through one lowest-common-denominator filesystem abstraction.

Suggested layout:

```text
cursedelete/
├── crates/
│   ├── cursdel-cli/
│   ├── cursdel-core/
│   ├── cursdel-policy/
│   ├── cursdel-license/
│   ├── cursdel-windows/
│   ├── cursdel-linux/
│   └── cursdel-macos/
├── tests/
│   ├── integration/
│   ├── destructive/
│   └── benchmark/
└── docs/
```

Shared components should include:

- CLI parsing;
- operation planning;
- bounded queues;
- adaptive concurrency controller;
- target validation;
- filtering;
- metrics;
- reporting;
- exit codes;
- licence/edition handling;
- audit event model;
- test contracts.

Platform-specific crates should own the actual deletion primitives.

---

# 5. Windows Native Engine

The Windows engine should use Win32 filesystem APIs directly where they provide a measurable advantage.

Candidate APIs and concepts include:

```text
FindFirstFileExW
FindNextFileW
FIND_FIRST_EX_LARGE_FETCH

CreateFileW

SetFileInformationByHandle
FileDispositionInfoEx

FILE_DISPOSITION_FLAG_DELETE
FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE

DeleteFileW
RemoveDirectoryW

GetFileAttributesW
SetFileAttributesW

FILE_FLAG_BACKUP_SEMANTICS
FILE_FLAG_OPEN_REPARSE_POINT

AdjustTokenPrivileges
SeTakeOwnershipPrivilege
SeBackupPrivilege
SeRestorePrivilege

SetNamedSecurityInfoW
ACL / security descriptor APIs
```

The engine should not blindly apply all possible remediation steps to every file.

Normal flow:

```text
attempt native delete
        |
        +--> success
        |
        +--> access denied
        |      |
        |      v
        |   remediation
        |      |
        |      v
        |     retry
        |
        +--> sharing violation
               |
               v
            lock handling
               |
               v
              retry
```

---

## 5.1 Windows Permissions and ACL Remediation

When authorised and explicitly permitted by the active operation mode, CurseDelete should be able to:

1. clear blocking file attributes;
2. attempt normal deletion again where appropriate;
3. take ownership when the current token has authority to do so;
4. repair or replace restrictive ACLs;
5. grant the required delete/full-control rights to the executing administrative context;
6. retry deletion.

The application should normally be run elevated for destructive/force modes.

It must never claim to bypass Windows security boundaries.

Correct product wording:

> CurseDelete automatically resolves file attributes, ownership and ACL restrictions where the executing security context has sufficient authority to do so.

Local Administrator privileges do not automatically grant rights over a remote SMB server.

---

## 5.2 Windows Long Paths and UNC Paths

CurseDelete should support:

```text
C:\folder
C:\folder\file

\\server\share\folder
\\server\share\folder\file
```

and internally use long-path-safe representations where required:

```text
\\?\C:\folder
\\?\UNC\server\share\folder
```

UNC support is a core product capability, not an afterthought.

---

## 5.3 Reparse Points, Junctions and Symlinks

CurseDelete must never unexpectedly escape the requested deletion boundary.

Example:

```text
C:\DeleteMe
└── Junction -> D:\ImportantData
```

Deleting `C:\DeleteMe` must never silently recurse into `D:\ImportantData`.

The implementation should:

- detect reparse points;
- operate on the reparse point itself where deletion is intended;
- avoid following it by default;
- expose any future follow behaviour only through an explicit dangerous option;
- validate target boundaries before processing;
- test junctions, symlinks, DFS behaviour, mount points and unusual filesystem drivers.

Safe failure is preferable to deleting outside the target boundary.

---

# 6. Linux Native Engine

The Linux engine should use native Linux/POSIX primitives rather than reproducing Windows behaviour.

Candidate primitives include:

```text
openat()
unlinkat()
fstatat()
getdents64() where benchmarking justifies it
chmod()/fchmodat()
```

The implementation should account for Unix permission semantics, including the fact that deleting a file is primarily governed by permissions on its parent directory.

The engine should:

- never follow symlinks outside the target boundary by default;
- support local and mounted filesystems exposed through the Linux filesystem namespace;
- use root privileges only where required;
- keep enumeration and deletion concurrent;
- use native metadata operations where they improve performance.

---

# 7. macOS Native Engine

The macOS engine should use Darwin/POSIX-native filesystem primitives.

It should not be a Windows engine translated into POSIX calls.

The macOS implementation should explicitly handle:

- POSIX deletion semantics;
- macOS ACLs;
- extended attributes where relevant;
- symlinks;
- mounted filesystems;
- filesystem-specific behaviour on APFS;
- permission remediation within the executing user's authority.

---

# 8. Target Safety Rules

CurseDelete is intentionally destructive, so target validation must be uncompromising.

The following must be rejected:

```text
C:\
D:\
E:\

\\server\share\

/
```

A valid destructive target must be at least one object below the filesystem/share root.

Examples:

```text
C:\folder
C:\folder\file.dat

\\server\share\folder
\\server\share\folder\file.dat

/var/tmp/build
/home/user/cache
```

Validation must happen after canonicalisation.

This must also be rejected:

```text
C:\folder\..
```

if it resolves to:

```text
C:\
```

Likewise, relative paths, symlinks, `..`, mount points, reparse points, case transformations, and UNC normalisation must never be able to turn an apparently valid child target into a protected root.

Additional safety policy may protect critical OS locations unless a separate, deliberately explicit override is introduced.

---

# 9. Enumeration + Deletion Pipeline

The strongest architecture from the previous CurseDelete implementation should be retained:

> **Delete while discovering.**

Do not:

```text
enumerate 10,000,000 paths
store all paths
then start deleting
```

Instead:

```text
enumerator
   |
   v
bounded queue
   |
   +--> worker
   +--> worker
   +--> worker
   +--> worker
```

The queue must provide backpressure so memory use remains bounded regardless of tree size.

Directories need separate lifecycle tracking so they can be deleted only once all relevant child work is complete.

---

# 10. Worker Model

Do not create one operating-system thread per file.

"Independent workers per file" means:

> Every file becomes an independent deletion work item processed by a bounded high-performance worker pool.

The implementation should support:

```text
--workers auto
--workers 4
--workers 16
--workers 64
--workers 128
```

`auto` should be the default.

---

# 11. Adaptive Worker Tuning

The optimum concurrency depends on the target.

Examples:

```text
local NVMe
local SATA SSD
local HDD
ReFS
NTFS
SMB over LAN
SMB over WAN
NAS
virtual disk
cloud-mounted share
```

CurseDelete should dynamically tune deletion concurrency.

The controller can observe:

- operations per second;
- average delete latency;
- latency percentiles;
- queue depth;
- CPU utilisation;
- filesystem errors;
- remote throttling;
- retry rate;
- throughput trend.

Conceptual strategy:

```text
start at conservative worker count
         |
         v
measure throughput
         |
         v
increase workers
         |
         +--> throughput improves -> continue
         |
         +--> throughput flat     -> hold
         |
         +--> throughput degrades -> back off
```

Manual `--workers` must disable automatic worker tuning.

Separate worker limits should be considered for:

- normal deletion;
- permission/ACL remediation;
- lock resolution;
- directory cleanup.

A permission-heavy workload must not stall ordinary deletions.

---

# 12. Locked Files

## 12.1 Local Windows Locks

Optional:

```text
--kill-locks
```

When a delete fails because of a sharing violation, CurseDelete may attempt to identify the process holding the file.

Preferred first implementation:

- Windows Restart Manager API.

Potential deeper implementation where justified:

- process/handle inspection using Windows-native handle APIs.

Conceptual flow:

```text
delete fails: sharing violation
        |
        v
identify lock holder
        |
        v
PID 4812: oldapp.exe
        |
        v
--kill-locks enabled?
        |
        v
terminate process
        |
        v
wait for handle release
        |
        v
retry deletion
```

Critical/system processes must be protected by policy.

CurseDelete must not terminate its own process.

Service-backed processes require special care because Windows Service Control Manager may restart them immediately.

A later implementation may distinguish between process termination and controlled service stop.

---

# 13. Remote SMB Locks

Network-share locks are different from local process locks.

Example:

```text
PC01 runs CurseDelete
        |
        v
\\FS01\Data\Old\foo.db
        ^
        |
PC02 has foo.db open
```

Killing processes on PC01 does not resolve the remote open.

For supported Windows file servers, CurseDelete may provide:

```text
--close-remote-locks
```

This feature should attempt to:

1. identify the SMB server;
2. query authorised open-file/session information on that server;
3. match the remote open to the requested path;
4. administratively close the remote file open;
5. retry deletion.

Requirements:

- appropriate administrative rights on the file server;
- remote-management capabilities available on the server;
- explicit user opt-in.

This must not be assumed to work universally on:

```text
Synology
NetApp
Samba
third-party NAS
cloud SMB implementations
```

For unsupported servers, CurseDelete should report the remote sharing violation clearly rather than pretend it can bypass it.

Closing remote opens can cause data loss for users/applications and should therefore remain an explicit feature.

---

# 14. Destructive Modes

Normal:

```bash
cursdel <path>
```

Aggressive permission remediation:

```bash
cursdel <path> --force
```

Local lock termination:

```bash
cursdel <path> --force --kill-locks
```

Remote Windows file-server open handling:

```bash
cursdel \\FS01\Builds\Old --force --close-remote-locks
```

A convenience mode may exist:

```bash
cursdel <path> --destroy
```

`--destroy` must have a precise documented meaning.

Suggested meaning:

```text
adaptive maximum-throughput deletion
+ permission/ownership/ACL remediation
+ attribute remediation
+ local lock termination
```

`--close-remote-locks` should remain separate because closing another user's server-side SMB open is materially different from local remediation.

---

# 15. Retention Cleanup

CurseDelete should support age-based deletion as a first-class use case.

Example:

```bash
cursdel C:\Logs --age 2d
cursdel \\FS01\Logs --age 90d
cursdel /var/cache/app --age 12h
```

A unit should be mandatory.

Supported durations should include:

```text
30m
12h
2d
4w
```

`--age 2` should be rejected as ambiguous.

Default age basis:

```text
modified time
```

Optional:

```bash
--age-by modified
--age-by created
--age-by accessed
```

Last-access timestamps are not reliable on every filesystem/configuration and must be documented accordingly.

---

## 15.1 Retention Directory Behaviour

With `--age`, the age filter applies to files.

Directories are deleted only when they are empty after processing.

Example:

```text
Logs/
├── old.log          8 days
├── yesterday.log    1 day
├── Ancient/
│   └── old.log      20 days
├── Current/
│   └── today.log    2 hours
└── Empty/
```

Running:

```bash
cursdel Logs --age 2d
```

results in:

```text
Logs/
├── yesterday.log
└── Current/
    └── today.log
```

`Ancient/` disappears because its contents qualified and it became empty.

`Empty/` disappears because it was empty.

Newer files are never deleted merely to remove their parent directory.

---

# 16. Additional Filters

Useful composable filters:

```bash
--include "*.log"
--exclude "*.keep"

--min-size 100m
--max-size 10g
```

Possible examples:

```bash
cursdel \\FS01\Logs --age 90d --include "*.log"
```

```bash
cursdel D:\BuildCache --age 14d --min-size 10m
```

Keep the filter model intentionally small.

Do not turn CurseDelete into a general filesystem query language.

---

# 17. Dry Run

Filtered deletion makes `--dry-run` mandatory.

Example:

```bash
cursdel \\FS01\Logs --age 90d --dry-run
```

Example output:

```text
CurseDelete

Target:       \\FS01\Logs
Mode:         retention
Age:          >= 90d
Age basis:    modified
Workers:      auto

Files scanned:       8,491,201
Directories:           183,422

Would delete:
  Files:             6,218,921
  Directories:         142,718
  Data:                1.84 TB

Would retain:
  Files:             2,272,280
  Directories:          40,704
  Data:              381.22 GB

No files were modified.
```

---

# 18. Output

Default console output should be useful but not noisy.

Example:

```text
CurseDelete 2.0

Target:       \\FS01\Builds\Old
Mode:         destroy
Workers:      auto -> 48
ACL repair:   enabled
Kill locks:   disabled
Remote locks: disabled

Scanning and deleting...

Files:          4,821,992
Directories:      182,441
Deleted:         891.4 GB
Failures:              0
Elapsed:         00:03:41
Rate:            21,819 files/sec

Complete.
```

Do not use joke status terms such as "banished", "cursed", or "sent to the void" in machine/production output.

The brand can be aggressive while the application remains professional.

---

# 19. Machine-Readable Output

Business and automation use cases need stable structured output.

Suggested options:

```bash
--json
--quiet
--log <path>
```

Potential later integrations:

```text
JSON Lines
CSV summary
Windows Event Log
syslog
OpenTelemetry
Prometheus-style metrics
```

Exit codes should be stable and documented.

Suggested starting contract:

```text
0   operation completed successfully
1   invalid arguments / rejected target
2   completed with one or more deletion failures
3   permission/privilege requirement not satisfied
4   lock resolution failed
5   remote lock handling failed
64  CLI usage error
99  unexpected fatal error
```

Exact codes should be frozen before the first stable release.

---

# 20. Performance Principles

Performance must be measured, not assumed.

Important principles:

1. use native platform APIs where benchmarking proves value;
2. minimise syscalls per file;
3. do not read metadata unless the operation requires it;
4. do not inspect ACLs unless normal deletion fails;
5. overlap enumeration and deletion;
6. bound memory usage;
7. tune worker counts dynamically;
8. keep remediation work out of the normal fast queue;
9. minimise console/log output during high-volume deletion;
10. treat network latency differently from local storage latency.

Verbose per-file logging can reduce performance dramatically and should be documented as such.

---

# 21. Benchmark Suite

Performance claims should be backed by a repeatable benchmark suite.

Compare at least:

```text
Windows:
- PowerShell Remove-Item
- cmd.exe rmdir /s /q
- robocopy empty-tree workaround where applicable
- previous CurseDelete C# implementation
- previous sfvdd Rust implementation
- CurseDelete 2

Linux:
- rm -rf
- find -delete where applicable
- CurseDelete 2

macOS:
- rm -rf
- CurseDelete 2
```

Benchmark scenarios should include:

### Local Windows

```text
1. 1,000,000 x 1 KB files on NVMe
2. 100,000 directories x 10 files each
3. extremely deep hierarchy
4. mixed file sizes
5. read-only files
6. restrictive ACLs
7. orphaned/broken permissions
8. junctions and symlinks
9. locked files
10. ReFS if available
```

### SMB

```text
1. 1,000,000 small files over low-latency LAN
2. high-latency SMB
3. Windows file server
4. third-party NAS
5. permission-heavy tree
6. remote file locks
7. DFS namespace where available
```

### Metrics

Capture:

```text
wall-clock time
files/sec
directories/sec
bytes represented
peak RSS
CPU
average latency
p95/p99 delete latency
retry count
remediation count
failed objects
worker count over time
```

Performance regressions should be tracked in CI where practical.

---

# 22. Security Model

CurseDelete is destructive by design.

Security requirements:

- never bypass the OS security model;
- never claim that local admin rights imply remote admin rights;
- explicit opt-in for process termination;
- explicit opt-in for remote SMB open closure;
- root/share-root deletion blocked structurally;
- symlink/reparse escape blocked;
- privilege use limited to the operation that requires it;
- machine-readable logs must not leak unnecessary secrets;
- no telemetry without explicit product policy and documentation;
- no silent destructive fallbacks.

---

# 23. CLI Proposal

Primary command:

```bash
cursdel <path> [options]
```

Core options:

```text
--force
--destroy

--workers auto
--workers <n>

--age <duration>
--age-by modified|created|accessed

--include <pattern>
--exclude <pattern>
--min-size <size>
--max-size <size>

--kill-locks
--close-remote-locks

--dry-run
--json
--quiet
--verbose
--log <path>

--version
--help
```

Examples:

```bash
cursdel C:\Temp\Old
```

```bash
cursdel C:\Temp\Old --force
```

```bash
cursdel C:\Temp\Old --destroy
```

```bash
cursdel C:\Temp\Old --workers 64
```

```bash
cursdel \\FS01\Builds\Old --force --workers auto
```

```bash
cursdel \\FS01\Logs --age 90d --dry-run
```

```bash
cursdel \\FS01\Logs --age 90d --include "*.log"
```

```bash
cursdel C:\OldApp --force --kill-locks
```

```bash
cursdel \\FS01\Archive --force --close-remote-locks
```

---

# 24. Edition Strategy

CurseDelete does **not** need eight separately engineered editions.

The recommended commercial structure is:

```text
Community
Business
Enterprise
```

Education can be provided as a **free licence class** with Community-like product capabilities and different legal eligibility/usage terms.

Consumer, Project, SMB, and Corporate are better treated as optional licence SKUs or pricing bands if commercial demand later justifies them.

The core deletion engine should remain excellent in every edition.

Do **not** artificially make Community slower.

A high-performance tool should win users because the free edition is genuinely good.

Paid editions should monetise commercial use, scale, automation, deployment rights, fleet/enterprise features, support, auditability, and advanced administrative operations.

---

# 25. Recommended Editions

## 25.1 Community Edition

**Price:** Free

**Target users:**

- personal use;
- developers;
- homelabs;
- open-source/community projects;
- evaluation.

**Suggested rights/capabilities:**

- full-speed deletion engine;
- local paths;
- UNC/network paths;
- adaptive workers;
- manual workers;
- standard permission/attribute remediation;
- retention cleanup;
- include/exclude filters;
- dry-run;
- standard console output;
- basic JSON output;
- local lock detection;
- no artificial performance limit.

Possible licence restrictions:

- non-commercial use;
- individual use;
- limited number of personal devices;
- community/open-source use.

Community should be good enough that technical users recommend the product.

---

## 25.2 Education Licence

**Price:** Free

Education is better treated as a licence class than a separate technical edition.

**Eligible use:**

- students;
- teachers;
- recognised schools/universities;
- classroom/lab use;
- non-commercial academic research.

Potential entitlement:

- same core capabilities as Community;
- multi-seat institutional classroom/lab allowance;
- no commercial administrative deployment unless separately licensed.

A university's commercial IT department should not automatically receive unlimited organisational deployment simply because the organisation is educational.

---

## 25.3 Business Edition

**Price:** Commercial

**Target users:**

- IT departments;
- software companies;
- DevOps teams;
- MSPs;
- file-server administrators;
- build/release infrastructure;
- SMB through mid-market organisations.

Include Community features plus business-use rights and features such as:

- commercial use;
- unattended/scheduled automation;
- CI/CD and build-agent use;
- Windows server use;
- fleet-friendly configuration;
- stable structured audit logs;
- JSON/JSONL reporting;
- policy/configuration files;
- Windows Event Log integration;
- central deployment rights;
- MSI/package-manager deployment assets where applicable;
- Intune/SCCM/GPO-friendly deployment;
- local process lock termination;
- supported Windows remote SMB open closure;
- enhanced diagnostics;
- support entitlement;
- offline licence activation where required.

Business licences may be sold by user, machine, server, or organisational band.

For CurseDelete, server/automation rights are likely more meaningful than simple named-user counts.

---

## 25.4 Enterprise Edition

**Price:** Custom / high-tier commercial

Include Business plus:

- organisation-wide deployment rights;
- unlimited or negotiated users/devices/servers;
- unlimited CI/build agents;
- air-gapped/offline licensing;
- enterprise policy configuration;
- enterprise audit output;
- central policy enforcement where implemented;
- SSO/central management if a management plane is ever introduced;
- priority support;
- security/compliance documentation;
- procurement-friendly invoicing and licence terms;
- negotiated subsidiaries/legal-entity coverage;
- long-term support arrangements where offered;
- deployment assistance;
- custom commercial terms.

Enterprise should primarily monetise **scale, operations, governance and support**, not additional deletion speed.

---

# 26. Optional Licence SKUs

These licence types can exist if market demand justifies them, but should not require separate binaries.

## Consumer

Potential use:

- paid personal use;
- one individual;
- several personally owned machines;
- commercial restrictions removed only where explicitly intended.

Likely unnecessary if Community already permits broad personal use.

**Recommendation:** do not launch initially unless a paid consumer tier is commercially useful.

---

## Project

Potential use:

- one named commercial project;
- small project team;
- limited machines/servers;
- automation permitted only for that project.

Useful for contractors or temporary migrations.

**Recommendation:** optional sales SKU, not a product edition.

---

## SMB

Potential use:

- smaller organisational licence;
- lower seat/server/device allowance than Business.

Example:

```text
25 users
50 endpoints
10 servers
```

**Recommendation:** price band under Business rather than a separate feature edition.

---

## Corporate

Potential use:

- large organisation;
- hundreds of users/endpoints;
- larger server/automation allowance;
- subsidiaries may be included.

**Recommendation:** pricing band between Business and Enterprise if sales demand requires it.

---

# 27. Edition Matrix

The exact commercial limits should be configurable outside the deletion engine.

Suggested initial model:

| Capability | Community | Education | Business | Enterprise |
|---|---:|---:|---:|---:|
| Full-speed deletion | Yes | Yes | Yes | Yes |
| Windows/Linux/macOS | Yes | Yes | Yes | Yes |
| Local paths | Yes | Yes | Yes | Yes |
| UNC/mounted shares | Yes | Yes | Yes | Yes |
| Adaptive workers | Yes | Yes | Yes | Yes |
| Manual workers | Yes | Yes | Yes | Yes |
| `--force` ACL/permission remediation | Yes | Yes | Yes | Yes |
| Age/retention cleanup | Yes | Yes | Yes | Yes |
| Dry-run | Yes | Yes | Yes | Yes |
| Basic JSON | Yes | Yes | Yes | Yes |
| Commercial use | No | No* | Yes | Yes |
| CI/build-agent automation | Limited/non-commercial | Academic | Yes | Yes |
| Local `--kill-locks` | Optional | Optional | Yes | Yes |
| Remote Windows `--close-remote-locks` | No | No | Yes | Yes |
| Advanced audit logging | No | No | Yes | Yes |
| Organisational deployment | No | Academic labs | Yes | Yes |
| Offline/air-gapped licensing | No | Optional | Optional | Yes |
| Priority support | No | No | Standard | Priority |
| Unlimited organisation scale | No | No | No | Negotiated/Yes |

\* Education use is subject to educational/non-commercial terms.

This matrix is a starting point, not a final legal licence.

---

# 28. Licensing Architecture

Edition/licence enforcement should be separate from core deletion logic.

Conceptually:

```text
Product
  |
  +--> licence
         |
         +--> edition
         +--> organisation/user
         +--> commercial-use rights
         +--> device/server limits
         +--> automation rights
         +--> optional capabilities
```

A signed licence could contain entitlements, but the deletion engine should consume a simple resolved policy object rather than understand commercial pricing.

Do not hard-code commercial prices into the executable.

---

# 29. Suggested Product Positioning

Short description:

> **CurseDelete is a native high-performance deletion utility for Windows, Linux and macOS. It deletes massive directory trees while it scans, automatically tunes concurrency, handles difficult permissions, supports network filesystems, and provides optional lock remediation for files that refuse to disappear.**

Windows-focused description:

> **A high-performance Windows deletion engine for local and UNC paths, designed for enormous directory trees, hostile ACLs, long paths, locked files, build infrastructure and file-server cleanup.**

Retention-focused description:

> **Use the same engine for high-speed retention cleanup: delete only files older than a policy threshold and remove directories only after they become empty.**

---

# 30. Non-Goals

CurseDelete should not become:

- a general file manager;
- a backup product;
- a secure data-erasure/forensic-wipe product unless separately designed for that purpose;
- a filesystem recovery tool;
- a permissions management suite;
- a general process manager;
- a full remote server management system;
- a cloud storage API client for every object-storage vendor;
- a replacement for lifecycle policies in object storage.

"Delete" means filesystem deletion according to the underlying filesystem semantics.

It does **not** imply cryptographic/forensic overwriting of physical storage blocks.

---

# 31. Build Guidance for an AI Coding Agent

An implementation agent such as Claude should treat this README as the architectural product brief.

Recommended implementation order:

## Phase 1 — Repository and contracts

1. create Rust workspace;
2. define CLI contract;
3. define shared result/error types;
4. define target canonicalisation contract;
5. define safety/root-protection tests;
6. define work item and bounded queue abstractions;
7. define platform engine interface without forcing low-level implementation details into a lowest-common-denominator abstraction.

## Phase 2 — Windows MVP

Implement Windows first because it has the most specialised product requirements.

Minimum Windows engine:

1. target validation;
2. long-path/UNC normalisation;
3. native directory enumeration;
4. bounded streaming pipeline;
5. parallel native file deletion;
6. safe directory deletion;
7. reparse-point safety;
8. manually configured worker count;
9. summary metrics;
10. integration/destructive tests.

## Phase 3 — Windows force mode

Add:

1. elevation detection;
2. token privilege enablement;
3. attribute remediation;
4. ownership remediation;
5. ACL remediation;
6. retry queue;
7. typed failure reporting.

Do not apply ACL operations before they are necessary.

## Phase 4 — Adaptive concurrency

Implement:

1. worker telemetry;
2. throughput sampling;
3. bounded worker scaling;
4. backoff;
5. stable manual `--workers` override;
6. benchmark-driven tuning.

## Phase 5 — retention and filters

Implement:

1. `--age`;
2. required duration units;
3. `--age-by`;
4. empty-directory cleanup;
5. include/exclude;
6. min/max size;
7. dry-run.

## Phase 6 — lock handling

Implement local Windows lock identification first.

Then:

1. `--kill-locks`;
2. protected-process rules;
3. retry behaviour;
4. service-awareness where needed.

Remote Windows file-server lock handling should be a separate later milestone.

## Phase 7 — Linux engine

Implement Linux-native enumeration/deletion and benchmark against `rm -rf`.

Do not copy Windows permission behaviour.

## Phase 8 — macOS engine

Implement Darwin-native behaviour and benchmark against `rm -rf`.

## Phase 9 — commercial capabilities

Only after the deletion engine is stable:

1. signed licence validation;
2. edition policy;
3. structured audit logging;
4. offline licensing;
5. enterprise deployment assets.

Do not couple licensing code into core filesystem code.

---

# 32. Engineering Rules for the AI Agent

The coding agent should follow these rules:

1. **Correctness before benchmark wins.**
2. **Never escape the requested target boundary.**
3. **Never permit root/share-root deletion.**
4. **Deletion starts before enumeration completes.**
5. **Memory use must remain bounded with arbitrarily large trees.**
6. **Do not perform expensive remediation on the normal successful path.**
7. **Do not follow links/reparse points by default.**
8. **Use native APIs where they have a demonstrated purpose.**
9. **Benchmark architectural changes.**
10. **Do not equate more workers with higher performance.**
11. **Manual worker count must be deterministic.**
12. **Automatic worker count must react to observed throughput.**
13. **Treat local and network filesystems differently where appropriate.**
14. **Keep machine-readable output stable.**
15. **Every destructive option must have integration tests.**
16. **Any remote-lock feature must require explicit opt-in.**
17. **Never hide failed deletions.**
18. **Do not silently downgrade a requested force operation.**
19. **No telemetry by default.**
20. **Keep platform-specific behaviour in platform-specific code.**

---

# 33. Test Requirements

At minimum, tests should cover:

```text
single file
single directory
large flat directory
deep tree
very large tree
empty directory
wildcards/patterns where supported
read-only file
hidden/system attributes on Windows
restricted ACL
ownership problem
long Windows path
UNC path
share-root rejection
drive-root rejection
Unix-root rejection
canonicalisation to root rejection
symlink
junction
directory reparse point
broken link
link to outside target
locked local file
retention keep/delete boundary
empty-folder cleanup
include/exclude
manual workers
automatic workers
dry-run
JSON output
partial failure
interrupted operation
```

Destructive tests must run in isolated temporary test roots.

Remote SMB tests should use disposable test shares.

---

# 34. Release Philosophy

The rewrite may be marketed as **CurseDelete 2**, but the CLI should remain:

```text
cursdel
```

Do not expose implementation generation in the command name.

Users should be able to upgrade without changing automation simply because the implementation moved from C# to Rust.

---

# 35. Recommended Initial Commercial Launch

Do not launch eight product editions.

Start with:

```text
Community
Business
Enterprise
```

and offer:

```text
Education
```

as a free eligibility/licence programme based on Community capabilities.

If sales later show demand, introduce pricing SKUs such as:

```text
Project
SMB
Corporate
```

without creating separate executables or fragmented feature sets.

Recommended principle:

> **The engine is fast for everyone. Organisations pay for commercial rights, scale, automation, enterprise administration, auditability, deployment and support.**

---

# 36. Product North Star

The benchmark goal is not merely:

> "faster than `rm -rf`."

It is:

> **For any supported filesystem target, CurseDelete should approach the highest practical deletion throughput the storage system can sustain while remaining bounded, predictable, auditable, permission-aware and contained to the requested target.**

The ideal administrator experience is:

```bash
cursdel \\FS01\Builds\Old --destroy
```

and the operator can trust that CurseDelete will:

1. validate the target;
2. refuse an unsafe root;
3. enumerate and delete concurrently;
4. tune workers to the target;
5. use the fastest native deletion path;
6. remediate deletable permission/ACL problems only when required;
7. optionally deal with lock holders when explicitly requested;
8. stay inside the requested tree;
9. clearly report anything it could not delete;
10. finish as fast as the underlying system realistically permits.

That is CurseDelete.
