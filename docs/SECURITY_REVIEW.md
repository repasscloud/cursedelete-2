# Security review

A focused security review was performed against the codebase as of commit
`06745a4` (core engine, macOS/Windows platform engines, licensing,
CLI — before the Linux engine and CI workflows landed; both will get an
equivalent pass once merged, as part of the mandated end-of-work
repo-wide review). Methodology: an independent identification pass across
the categories below, followed by adversarial false-positive filtering on
every candidate finding before anything was accepted or fixed.

## Areas reviewed

- **Command injection**: every subprocess call in the codebase (`lsof`,
  `ioreg`, `hostname` in `cursdel-macos`/`cursdel-license`; `touch` in
  test code) uses argv-array `std::process::Command`, never a shell.
  No finding.
- **Path traversal / root protection**: `cursdel-core::target`'s two-layer
  defence (lexical `..`-collapse detection + real filesystem
  canonicalisation) was re-examined against how paths actually flow from
  CLI argument through to each platform engine's delete syscalls. No
  bypass found beyond the two TOCTOU gaps already disclosed and accepted
  in [ADR-0006](adr/0006-posix-toctou.md) and
  [ADR-0007](adr/0007-windows-engine.md).
- **License signature verification**: format/algorithm are hardcoded
  checks, the signing key must already be present in the compiled-in
  trust store (no way to smuggle in an attacker key), canonical-JSON
  bytes are computed correctly over the envelope minus the signature
  field, and ECDSA-P256/IEEE-P1363 verification is standard and correctly
  wired. No finding.
- **`--close-remote-locks` license gate**: single enforcement point in
  `cursdel-cli::main`, independently re-verifying the on-disk licence
  signature on every invocation; unrecognised edition strings fail closed
  to Community. No bypass found.
- **Windows ownership/ACL remediation and process termination**: ownership
  is always taken in the name of the *current process token's user*, never
  a hardcoded Administrators SID; `SetNamedSecurityInfoW`'s real
  success/failure is always what's reported. `--kill-locks`'s
  name-check-then-terminate sequence was examined for a PID-reuse TOCTOU
  (two independent `OpenProcess` calls rather than one held handle); ruled
  a theoretical rather than practically exploitable race after
  adversarial review — see "Findings not accepted" below.
- **Cryptographic randomness**: the 32-byte activation bearer token is
  generated via `rand::thread_rng()` (OS-entropy-seeded CSPRNG),
  appropriate for a bearer credential.
- **Secret exposure in output**: traced every code path where
  `ActivationCredentials`/`activation_token` flows
  (`store.rs`/`client.rs`/`license_cmd.rs`); no print/log statement
  surfaces it. `license status`/`activate` print license ID, customer,
  edition, and activation ID only.

## Finding fixed

**Activation-token file permissions not re-enforced on an already-existing
file** (`crates/cursdel-license/src/store.rs`, `write_restrictive`).
POSIX's `open(..., O_CREAT, mode)` only applies the requested `mode` when
the call actually creates the file; if `activation.json` already existed
at that path (left behind by an older version, a backup/restore tool that
doesn't preserve exact mode bits, or a manual copy) with looser
permissions, every subsequent `cursdel license activate`/`refresh` would
silently keep writing the bearer token into that file at its pre-existing
permissions rather than the intended owner-only `0600`. This was a
deterministic logic gap, not a race condition -- fully reproducible, not
timing-dependent.

**Fix**: `write_restrictive` now calls `set_permissions(0o600)`
explicitly and unconditionally after opening the file, regardless of
whether the file was just created or already existed. Regression test:
`store::tests::activation_credentials_permissions_are_restored_on_a_preexisting_loose_file`,
which pre-creates the file at `0644` before calling
`save_activation_credentials` and asserts the result is `0600`.

## Findings not accepted (theoretical, not concretely exploitable)

**Windows `--kill-locks` PID-reuse TOCTOU** between the executable-name
protected-process check (`resolve_exe_basename`, which opens, queries, and
closes a handle) and the subsequent `TerminateProcess` call
(`terminate_process`, which opens an independent handle by PID) in
`crates/cursdel-windows/src/lock.rs`. Real gap in the code (no single
handle is held across both steps), but adversarial review concluded the
race window is microsecond-scale CPU-only work with no attacker-observable
synchronisation point, requiring an attacker to both win a hostile-timing
race *and* have the OS's PID allocator (which they don't control) hand
the exact freed PID to a specific victim process -- winning both
simultaneously is not a practically executable attack, distinguishing it
from documented real-world PID-reuse exploits that typically rely on much
longer-lived cached PIDs or attacker-controllable allocation timing.
Recorded here rather than silently dropped so the reasoning is visible;
worth revisiting if Windows validation surfaces a more concrete path, and
holding a single handle across both steps (rather than two independent
`OpenProcess` calls) would close it cheaply if ever judged worthwhile as
defence-in-depth.

## Accepted residual risks (already documented, not new findings)

- **POSIX ancestor-chain TOCTOU** ([ADR-0006](adr/0006-posix-toctou.md)):
  reopening a delete target's immediate parent by path (rather than a
  fully `openat`-chained walk from the root) closes the final-component
  symlink-swap race but not a race against an *ancestor* directory being
  replaced mid-walk. Requires local code execution with write access
  inside the target tree.
- **Windows analogous gap** ([ADR-0007](adr/0007-windows-engine.md)):
  same shape, `NtCreateFile`'s `OBJECT_ATTRIBUTES.RootDirectory` handle-
  chaining is the equivalent fix, not yet implemented for the same
  reason.
- Both are also flagged in [docs/BENCHMARKS.md](BENCHMARKS.md) as the
  same code change that would additionally fix a measured performance gap
  on deep trees -- tracked as the top follow-up item for the engine
  architecture, not committed to in this pass.

## Explicitly out of scope for this pass

Denial-of-service/resource exhaustion, memory safety (impossible in safe
Rust; `unsafe` blocks in the Windows engine were reviewed for logic
correctness during the Windows engine implementation and its review, not
re-litigated here), secrets-on-disk questions where the storage mechanism
itself is sound (only a concrete gap in the mechanism was in scope, which
is exactly what the accepted finding above is), and anything requiring a
live Windows session or real SMB/Restart-Manager target this environment
cannot provide (tracked via `TODO(windows-ci)` comments in the Windows
engine crate).
