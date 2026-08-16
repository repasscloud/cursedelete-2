# CursedDelete

A fast, parallel file and directory deleter for Windows, Linux, and macOS. Built for cleaning up large trees — millions of files, deep hierarchies, locked paths, and network shares — where standard tools are too slow or fail silently.

## Installation

Build from source using the .NET 10 SDK:

```bash
dotnet publish -r win-x64 -c Release
```

Swap the RID for your platform:

| Platform       | RID            |
|----------------|----------------|
| Windows x64    | `win-x64`      |
| Windows ARM64  | `win-arm64`    |
| Linux x64      | `linux-x64`    |
| Linux ARM64    | `linux-arm64`  |
| macOS x64      | `osx-x64`      |
| macOS ARM64    | `osx-arm64`    |

Output lands in `bin/Release/net10.0/<rid>/publish/`. The binary is self-contained and requires no .NET runtime on the target machine.

## Usage

```none
cursedelete <path>
cursedelete <path> -recurse
cursedelete <path> -force
cursedelete <path> -recurse -force -v -dop <number>
cursedelete version
```

## Examples

```bash
# Delete a single file
cursedelete C:\Temp\file.txt

# Delete an entire directory tree
cursedelete C:\Temp\OldFolder

# Delete all .log files in a directory
cursedelete C:\Logs\*.log

# Delete all .log files recursively, skipping junctions and symlinks
cursedelete C:\Logs\*.log -recurse

# Force-delete a locked tree as Administrator (Windows)
cursedelete C:\Temp\LockedFolder -force -v

# Force-delete on a network share with high parallelism
cursedelete \\server\share\drop -recurse -force -dop 48

# Linux / macOS
sudo cursedelete /var/old-data -recurse -force -v

# Show version and environment info
cursedelete version
```

## Flags

| Flag | Description |
| --- | --- |
| `-recurse` | Recurse into subdirectories. For wildcard targets, applies the pattern to all subdirectories. For directory targets, deletes the entire tree. |
| `-force` | Take ownership, reset ACLs, clear file attributes, then retry any failed delete. Requires Administrator on Windows or root on Linux/macOS. |
| `-v` | Verbose output. Prints every file and directory deleted, every skip, and typed error diagnostics on failure. |
| `-dop <n>` | Set the degree of parallelism — how many files are deleted concurrently. See section below. |

## How It Works

### Execution paths

CursedDelete has three distinct execution paths depending on what you pass as the target:

**Single file** — deletes it directly, with an optional force-unlock retry if it fails.

**Wildcard pattern** (path contains `*` or `?`) — matches files only. Directory names are never matched by the pattern; if you want to delete a whole tree, pass the directory path directly. When `-recurse` is used, CursedDelete walks subdirectories itself using an explicit stack rather than delegating to `SearchOption.AllDirectories`, so reparse point skipping applies consistently.

**Directory path** — deletes the entire tree. Files are deleted first, then directories are removed deepest-first (reverse discovery order) so that no directory delete is attempted while it still has children.

### Producer/consumer parallelism

Both the wildcard and directory tree paths use a bounded `BlockingCollection` queue. One producer thread walks the filesystem and feeds paths into the queue. Multiple consumer worker threads drain the queue and perform the actual deletes in parallel.

The key property of this design is that **deletion begins immediately** as paths are discovered, rather than waiting for the entire tree to be enumerated first. For a tree with ten million files this matters a lot — you delete while you walk rather than loading all ten million paths into memory before touching a single file. The bounded queue capacity also provides backpressure: the producer pauses when the workers fall behind, keeping memory flat regardless of tree size.

### Reparse point safety

On Windows, before pushing any subdirectory onto the walk stack, CursedDelete checks whether it is a reparse point (junction or symlink). If it is, the entry is skipped entirely and never recursed into. This prevents accidentally walking outside the intended root, hitting infinite loops, or deleting content in unrelated locations that a junction points to.

If the attribute read itself fails, the entry is treated as a reparse point and skipped — the safe default.

Note: reparse points that are skipped are not deleted. If a directory contains a junction, that directory will likely fail to delete since it is not empty. The failure is reported in the summary count and, with `-v`, includes a typed error category.

### Force unlock sequence (Windows)

When `-force` is set and a delete fails, CursedDelete runs the following sequence before retrying:

1. **Take ownership** — sets the current user as the owner of the file or directory via the Windows security API, which is a prerequisite for being able to modify the ACL at all.
2. **Reset ACL** — disables ACL inheritance, removes all existing rules, and writes a fresh ACL granting `FullControl` to Administrators, SYSTEM, and the current user. This replaces any deny rules or restrictive permissions that were blocking the delete.
3. **Clear attributes** — sets file attributes to `Normal` (files) or clears the `ReadOnly` flag (directories), removing hidden/system/read-only flags that can also block deletion.
4. **Retry delete** — attempts the delete again with the unlocked path.

Both the ownership and ACL steps report success or failure independently. If either step fails, a warning is logged and the delete is still attempted — partial unlocks sometimes succeed.

`-force` requires Administrator on Windows. It does not bypass OS access control boundaries; it operates within the authority of the elevated user.

### Force unlock (Linux / macOS)

On Unix, `-force` runs `chmod u+rwx` on the target path before retrying. Root is required (`sudo`). Since file deletion on Unix is controlled by the parent directory's write permission rather than the file's own permissions, the chmod is applied to the parent directory for files.

### Elevation check

On Windows, elevation is checked via `WindowsPrincipal.IsInRole(Administrator)`.

On Linux and macOS, elevation is checked via `geteuid() == 0` through a P/Invoke call to `libc`. This correctly handles `sudo`, `setuid` binaries, and any scenario where the effective user ID differs from the login name — unlike checking `$USER == root`.

## Degree of Parallelism (`-dop`)

The `-dop` flag controls how many files are deleted concurrently by the worker threads.

The default scales with your CPU count. For I/O-bound workloads (which file deletion always is), running more threads than cores is beneficial because each thread spends most of its time waiting on the filesystem or network, not burning CPU.

**When to tune `-dop`:**

| Scenario | Recommendation |
| --- | --- |
| Local NVMe SSD | Default is usually fine. Try `-dop 8` to `-dop 16`. |
| Local HDD / spinning rust | Lower is sometimes better. HDDs suffer from seek contention with high parallelism. Try `-dop 4`. |
| Fast LAN network share | Increase significantly. Latency per operation is high so more threads keep the pipe full. Try `-dop 32` to `-dop 64`. |
| High-latency WAN / cloud share | Go higher. Each delete round-trips across the network. Try `-dop 64` or more. |
| Server under load | Lower to reduce impact on other workloads. `-dop 2` or `-dop 4`. |

There is no hard upper limit enforced beyond what you specify. If you set `-dop 256` on a WAN share, CursedDelete will run 256 concurrent workers. Whether that helps or saturates the remote server depends on the environment.

## Exit Codes

| Code | Meaning |
| --- | --- |
| `0` | All targets deleted successfully. |
| `1` | Bad arguments or target not found. |
| `2` | Completed with one or more failures. |
| `99` | Unexpected fatal error. |

## Network Shares

CursedDelete works against UNC paths (`\\server\share\folder`) and standard network mounts. A few things to be aware of:

`-force` is limited on network shares. Ownership and ACL operations go through local Windows APIs and whether they succeed against a remote path depends on your permissions on the remote machine, not just local elevation.

Reparse point detection over SMB is not always reliable. DFS links and network mount points may not surface the `ReparsePoint` attribute flag correctly depending on the server OS and SMB version.

High `-dop` values are recommended for network targets since each operation blocks on network round-trip time. Start with `-dop 32` and increase from there.

## Requirements

- .NET 10 SDK to build
- Windows, Linux, or macOS
- Administrator / root only required when using `-force`
