# Release process

This document describes how a CurseDelete 2 release is cut, what
`.github/workflows/release.yml` produces automatically, and what is still
manual or missing.

## Cutting a release

1. Land all changes for the release on `main` (via `dev` → `main`, per the
   repo's normal branch flow).
2. Bump `version` under `[workspace.package]` in the root `Cargo.toml` to the
   new version (every crate in the workspace inherits this version, so this
   is the single place to change it). Commit that change.
3. Update `CHANGELOG.md`: move the relevant `Unreleased` entries under a new
   `## [X.Y.Z] - YYYY-MM-DD` heading.
4. Tag the commit `vX.Y.Z` (matching the `Cargo.toml` version exactly,
   including no `v` prefix in the `Cargo.toml` value) and push the tag:

   ```sh
   git tag v2.1.0
   git push origin v2.1.0
   ```

5. Pushing a `v*` tag triggers `.github/workflows/release.yml`. Its first
   job, `check-version`, fails the whole workflow immediately (before
   spending any build minutes) if the pushed tag doesn't match
   `Cargo.toml`'s version -- if that happens, delete the tag, fix the
   version, and re-tag.
6. On success, a GitHub Release is created for the tag (or reused, via
   `softprops/action-gh-release`, if one already exists) with all build
   artifacts and `SHA256SUMS.txt` attached, and an auto-generated changelog
   body (`generate_release_notes: true`) that you should still hand-edit to
   link the corresponding `CHANGELOG.md` section.

## What the release workflow produces

For each of the six shipping targets:

| Target | Runner | How it's built |
|---|---|---|
| `x86_64-pc-windows-msvc` | `windows-latest` | native `cargo build --release` |
| `aarch64-pc-windows-msvc` | `windows-latest` | cross-compiled `cargo build --release --target aarch64-pc-windows-msvc`, using the ARM64 MSVC cross tools bundled with the runner's Visual Studio install |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native `cargo build --release` |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | [`cross`](https://github.com/cross-rs/cross) (Docker-based cross toolchain), `cross build --release --target aarch64-unknown-linux-gnu` |
| `x86_64-apple-darwin` | `macos-latest` (Apple Silicon) | cross-compiled `cargo build --release --target x86_64-apple-darwin` (Xcode's clang is multi-arch by default, no extra toolchain needed) |
| `aarch64-apple-darwin` | `macos-latest` (Apple Silicon) | native `cargo build --release` |

Every binary is built with the workspace's `[profile.release]` in
`Cargo.toml` (`strip = "symbols"`, `lto = "thin"`, `codegen-units = 1`), so
release binaries are already stripped as part of the normal build -- no
separate strip step is needed in the workflow.

Each target's binary (`cursdel` / `cursdel.exe`) is packaged with the repo's
`LICENSE` and a copy of `README-CurseDelete2.md` (renamed to `README.md`
inside the archive) into:

- `cursdel-<version>-<target>.zip` for Windows targets, built with
  PowerShell's `Compress-Archive` (no extra tooling required on the
  runner).
- `cursdel-<version>-<target>.tar.gz` for Linux/macOS targets.

A single `SHA256SUMS.txt` covering every artifact is generated in the final
`release` job and uploaded alongside the archives.

## Known risk: Windows ARM64 build tools

The `aarch64-pc-windows-msvc` leg depends on GitHub's `windows-latest`
runner image actually having the ARM64 MSVC v143 build tools installed.
This has had real, documented gaps in the past
([actions/runner-images#5056](https://github.com/actions/runner-images/issues/5056))
and, as of this writing, there is an **open, unresolved** issue on the
newer Windows Server 2025 / VS2026 runner image specifically
([actions/runner-images#14215](https://github.com/actions/runner-images/issues/14215)).

If the `build-windows` / `aarch64-pc-windows-msvc` job (or the equivalent
`cross-check` job in `ci.yml`) starts failing with a missing ARM64
linker/`cl.exe` error that isn't caused by a change in this repo:

1. Check whether the linked upstream issue (or its GitHub-runner-images
   successor) is still open.
2. As a workaround, pin the job's `runs-on` to a specific older Windows
   runner image label known to have the component (e.g. `windows-2022`
   instead of `windows-latest`) until GitHub resolves the gap upstream, or
   add an explicit Visual Studio component install step
   (`Microsoft.VisualStudio.Component.VC.Tools.ARM64`) before the build.
   This was deliberately **not** done preemptively in `release.yml` because
   it could not be verified to work from this development environment
   (no live Windows Actions runner available) -- don't guess at a fix for a
   problem that may not exist on the runner image in use at release time.

## What's manual / missing (follow-up work)

None of the following is implemented. Each is a real gap that should be
tracked and picked up separately, not something this pass fabricated a
placeholder for beyond documenting the requirement.

### Windows Authenticode code signing

`build-windows` in `release.yml` has an inert placeholder step ("Authenticode
sign") that does nothing today and only exists to fail loudly if someone
adds a `WINDOWS_CERTIFICATE` secret without also implementing real signing
(so a half-configured secret can't create a false sense of security).

To implement real signing:

- Obtain a code-signing certificate (an EV certificate is strongly
  recommended -- it avoids Windows SmartScreen reputation warnings that a
  standard OV certificate will still trigger for a new publisher).
- Add repo secrets: `WINDOWS_CERTIFICATE` (base64-encoded `.pfx`) and
  `WINDOWS_CERTIFICATE_PASSWORD`.
- Add a step that decodes the certificate to a temp file and invokes
  `signtool.exe sign /f <cert> /p <password> /fd sha256 /tr
  <timestamp-url> /td sha256 target\<target>\release\cursdel.exe` before
  packaging. `signtool.exe` ships with the Windows SDK, already present on
  `windows-latest` runners.
- Delete the temp certificate file in an `if: always()` cleanup step.

### macOS codesigning and notarization

`build-macos` has the equivalent inert placeholder ("Codesign & notarize").

To implement:

- Enroll in the Apple Developer Program and obtain a "Developer ID
  Application" certificate.
- Add repo secrets: `APPLE_CERTIFICATE` (base64-encoded `.p12`),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`, and an
  app-specific password (`APPLE_APP_PASSWORD`) for `notarytool`.
- Import the certificate into a temporary keychain, `codesign --sign
  "Developer ID Application: ..." --options runtime target/<target>/release/cursdel`,
  then `xcrun notarytool submit` + `xcrun stapler staple` (stapling a
  bare CLI binary, rather than a `.app`/`.dmg`, needs the binary shipped
  inside a signed `.dmg` or `.pkg` for `staple` to attach to -- decide on
  a distribution container format as part of this work, not just the
  signing step).

### Windows MSI packaging and WinGet submission

Currently, Windows ships as a plain `.zip` of `cursdel.exe`, as the task
brief allows for this pass. Follow-up work:

- **MSI**: build with [WiX Toolset](https://wixtoolset.org/) (v4/v5, via
  the `wix` CLI, installable via `dotnet tool install --global wix` on the
  `windows-latest` runner) or
  [`cargo-wix`](https://github.com/volks73/cargo-wix). This needs a real
  `.wxs` manifest (upgrade codes, shortcuts, PATH registration decisions)
  authored and tested against an actual Windows machine or a live Windows
  Actions run -- not attempted here because it could not be verified from
  this development environment, and a subtly wrong WiX manifest (e.g. a
  reused component GUID) can produce an installer that silently fails to
  upgrade cleanly on top of a previous install.
- **WinGet**: once an MSI (or a signed `.exe` installer) exists, submit a
  manifest to
  [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) (a
  `<Publisher>.<Product>` manifest with `InstallerType: msi`, a SHA256 of
  the installer, and license metadata). WinGet requires the installer be
  code-signed for smooth submission review, so this depends on Authenticode
  signing landing first. The `winget-create` CLI tool
  (`wingetcreate new <url-to-msi>`) can scaffold the manifest once the
  signed MSI is published to a release.

### Linux package-manager distribution

No `.deb`/`.rpm`/Homebrew formula is produced. Follow-up work, each with
its own validation target this pass didn't have access to:

- **Homebrew**: a formula (or, more simply, a
  [Homebrew tap](https://docs.brew.sh/Taps)) pointing at the
  `x86_64-apple-darwin` / `aarch64-apple-darwin` release tarballs and their
  SHA256 sums from `SHA256SUMS.txt`. Straightforward once there's a stable
  release cadence; doesn't need Apple's own toolchain to build (Homebrew
  formulas typically just download+verify the prebuilt tarball for a CLI
  tool like this, or build from source with `cargo install` -- the latter
  would need `LICENSE`/crates published or a `url`+`sha256` `stable` block
  pointing at a git tag).
- **`.deb`/`.rpm`**: [`cargo-deb`](https://github.com/kornelski/cargo-deb)
  and [`cargo-generate-rpm`](https://github.com/cat-in-136/cargo-generate-rpm)
  are the standard tools; both need a maintained package repository
  (e.g. a PPA, or self-hosted apt/yum repo with GPG-signed metadata) to be
  useful beyond a manually-downloaded `.deb`/`.rpm` file, which is a
  meaningfully larger piece of infrastructure than this pass's scope
  (workflow YAML + packaging scripts for the existing GitHub Release flow).

## Benchmarks are intentionally not part of any release or CI gate

`README-CurseDelete2.md` describes a benchmark suite (`tests/benchmark/` in
its sketched layout, section 21 "Benchmark Suite"), but no runnable
benchmark harness exists in the repository yet. Per this task's brief,
performance benchmarking must never gate CI/release on shared or virtual
runners (noisy, non-representative hardware), and no placeholder workflow
was added for it since there is nothing runnable to invoke yet. When a real
benchmark harness lands under `tests/benchmark/`, add a separate
`workflow_dispatch`-only workflow (e.g. `.github/workflows/benchmark.yml`)
that runs it on demand and is clearly documented as informational-only,
never required for merge.
