//! Destructive-target validation and root/share-root protection.
//!
//! CurseDelete must never be able to delete a filesystem root or an SMB
//! share root, and a path must not be able to disguise itself as one via
//! `..` traversal or a symlink/reparse point. This module is the single
//! choke point that every entry path (CLI target, and -- defensively --
//! any path an engine is about to hand to a native delete call) passes
//! through.
//!
//! Two independent layers cooperate:
//!
//! 1. A **pure lexical parser** (`classify`, `LexicalPath`) that
//!    understands Windows drive paths (`C:\...`), Windows UNC/share paths
//!    (`\\server\share\...`, including the `\\?\` and `\\?\UNC\` verbatim
//!    forms), and POSIX absolute paths (`/...`) purely as strings, with no
//!    filesystem access. This exists because `std::path::Path` parses
//!    components using the *host* OS's separator rules -- on Unix, `\` is
//!    just an ordinary filename character, so a Windows-style string is a
//!    single opaque component to `Path`. The lexical parser lets the full
//!    Windows/UNC safety matrix be exercised in unit tests on any host
//!    (including this repository's macOS/Linux development and CI
//!    environments), and it alone is enough to catch a literal root or a
//!    `..` collapse such as `C:\folder\..` -> `C:\`.
//!
//! 2. **Filesystem canonicalisation** (`validate_target`), which resolves
//!    symlinks/junctions in the requested path via the OS's own
//!    canonicalisation so that a symlink used to *disguise* a root (e.g. a
//!    junction whose real target is the drive root) is caught too. This
//!    layer necessarily behaves differently per host OS and is exercised
//!    with real temporary directories in tests.
//!
//! These two layers answer different questions and are deliberately not
//! merged: layer 1 asks "does the literal argument denote a root", layer 2
//! asks "does the argument *resolve* to a root". Neither implies the
//! enumerator should follow symlinks found *inside* the target tree during
//! deletion -- that is a separate, narrower guarantee implemented in the
//! platform engines (see `docs/adr/0005-symlink-reparse-safety.md`).

use std::path::{Path, PathBuf};

use crate::error::TargetError;

/// The recognised absolute-path prefix forms.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Prefix {
    /// `C:` (from `C:\...`, `C:/...`, or the verbatim `\\?\C:\...` form).
    Drive(char),
    /// `\\server\share` (from `\\server\share\...` or the verbatim
    /// `\\?\UNC\server\share\...` form).
    Unc { server: String, share: String },
    /// A POSIX-style absolute path starting with `/`.
    PosixAbsolute,
    /// No recognised absolute prefix; the path is relative.
    Relative,
}

/// The result of lexically parsing and `.`/`..`-resolving a path string,
/// independent of any real filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LexicalPath {
    prefix: Prefix,
    /// Path components below the prefix, after resolving `.` and `..`.
    components: Vec<String>,
}

impl LexicalPath {
    /// True if this path denotes exactly the root/share described by its
    /// prefix, with nothing below it -- i.e. it is a protected root.
    fn is_protected_root(&self) -> bool {
        match &self.prefix {
            Prefix::Drive(_) => self.components.is_empty(),
            Prefix::Unc { .. } => self.components.is_empty(),
            Prefix::PosixAbsolute => self.components.is_empty(),
            Prefix::Relative => false,
        }
    }

    /// Re-render this path as a display string for error messages.
    fn display_string(&self) -> String {
        match &self.prefix {
            Prefix::Drive(letter) => {
                let mut s = format!("{letter}:\\");
                s.push_str(&self.components.join("\\"));
                s
            }
            Prefix::Unc { server, share } => {
                let mut s = format!("\\\\{server}\\{share}\\");
                s.push_str(&self.components.join("\\"));
                s
            }
            Prefix::PosixAbsolute => format!("/{}", self.components.join("/")),
            Prefix::Relative => self.components.join("/"),
        }
    }
}

/// Split `s` on both `/` and `\`, since Windows accepts either separator in
/// most APIs and a malicious/careless input may mix them.
fn split_any_separator(s: &str) -> Vec<&str> {
    s.split(['/', '\\']).collect()
}

/// Lexically parse and resolve `raw` into a [`LexicalPath`]. `raw` should
/// already be an absolute path string (callers are responsible for joining
/// relative input onto a base directory first); a relative `raw` is still
/// handled, but leading `..` components that would escape the available
/// prefix are kept literally since there is no lexical answer for them.
fn classify(raw: &str) -> LexicalPath {
    // Strip a `\\?\` / `//?/` verbatim prefix (case-insensitive), which may
    // be followed by `UNC\` for the UNC verbatim form.
    let (verbatim_unc, rest) = strip_verbatim_prefix(raw);

    let (prefix, remainder) = if verbatim_unc {
        parse_unc_body(rest)
    } else if let Some(body) = strip_double_separator(rest) {
        parse_unc_body(body)
    } else if let Some(drive) = drive_letter(rest) {
        (Prefix::Drive(drive), &rest[2..])
    } else if rest.starts_with(['/', '\\']) {
        (Prefix::PosixAbsolute, rest)
    } else {
        (Prefix::Relative, rest)
    };

    let components = resolve_dot_components(split_any_separator(remainder));

    LexicalPath { prefix, components }
}

fn strip_verbatim_prefix(raw: &str) -> (bool, &str) {
    for prefix in ["\\\\?\\", "//?/", "\\\\?/", "//?\\"] {
        if let Some(rest) = strip_prefix_ci(raw, prefix) {
            if let Some(unc_rest) =
                strip_prefix_ci(rest, "UNC\\").or_else(|| strip_prefix_ci(rest, "UNC/"))
            {
                return (true, unc_rest);
            }
            return (false, rest);
        }
    }
    (false, raw)
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// If `s` starts with two consecutive separators (a non-verbatim UNC path,
/// e.g. `\\server\share`), return the remainder after them.
fn strip_double_separator(s: &str) -> Option<&str> {
    let mut chars = s.chars();
    let a = chars.next()?;
    let b = chars.next()?;
    if matches!(a, '/' | '\\') && matches!(b, '/' | '\\') {
        Some(&s[2..])
    } else {
        None
    }
}

fn drive_letter(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    if chars.next() != Some(':') {
        return None;
    }
    Some(letter.to_ascii_uppercase())
}

fn parse_unc_body(body: &str) -> (Prefix, &str) {
    let parts = split_any_separator(body);
    let server = parts.first().copied().unwrap_or("").to_string();
    let share = parts.get(1).copied().unwrap_or("").to_string();

    // Find the byte offset just past `server\share` so we can hand back the
    // true remainder for component splitting.
    let mut offset = 0usize;
    let mut seen = 0usize;
    for (i, ch) in body.char_indices() {
        if matches!(ch, '/' | '\\') {
            seen += 1;
            if seen == 2 {
                offset = i + ch.len_utf8();
                break;
            }
        }
    }
    let remainder = if seen >= 2 { &body[offset..] } else { "" };

    (Prefix::Unc { server, share }, remainder)
}

fn resolve_dot_components(parts: Vec<&str>) -> Vec<String> {
    let mut stack: Vec<String> = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            "" | "." => continue,
            ".." => {
                // Absorb at the root, matching real filesystem semantics
                // (`/..` == `/`). This is intentional: it is exactly the
                // behaviour that must be caught as "resolves to root".
                stack.pop();
            }
            other => stack.push(other.to_string()),
        }
    }
    stack
}

/// A destructive target that has passed root/share-root validation and
/// filesystem canonicalisation.
#[derive(Debug, Clone)]
pub struct CanonicalTarget {
    /// The path exactly as the user supplied it (used for display and for
    /// the actual delete syscalls, so that a symlink target is deleted as
    /// a link rather than silently followed).
    pub requested: PathBuf,
    /// The fully resolved, symlink-free, absolute path used only for
    /// safety comparisons.
    pub canonical: PathBuf,
    /// True if the requested path itself (its final component) is a
    /// symlink/reparse point rather than a regular file or directory.
    pub is_symlink: bool,
    pub is_dir: bool,
}

/// Validate `path` as a destructive target: it must exist, and it must not
/// be -- or lexically/symbolically resolve to -- a filesystem root or SMB
/// share root.
pub fn validate_target(path: &Path) -> Result<CanonicalTarget, TargetError> {
    if path.as_os_str().is_empty() {
        return Err(TargetError::EmptyPath);
    }

    let raw = path.to_string_lossy().into_owned();

    // Layer 1: pure lexical rejection, including the `..`-collapse attack.
    // Relative inputs are joined onto the current directory first so the
    // lexical resolver always has an absolute string to work with.
    let absolute_raw = if classify(&raw).prefix == Prefix::Relative {
        let cwd = std::env::current_dir().map_err(|source| TargetError::Canonicalize {
            path: path.to_path_buf(),
            source,
        })?;
        cwd.join(path).to_string_lossy().into_owned()
    } else {
        raw.clone()
    };

    let lexical = classify(&absolute_raw);
    if lexical.is_protected_root() {
        return Err(reject_root(path, &lexical, &lexical));
    }

    // Layer 2: real filesystem canonicalisation, to catch a symlink/
    // junction that *resolves* to a root even though the literal argument
    // does not.
    let absolute_path = PathBuf::from(&absolute_raw);
    let leaf_meta = std::fs::symlink_metadata(&absolute_path)
        .map_err(|_| TargetError::NotFound(path.to_path_buf()))?;
    let is_symlink = leaf_meta.file_type().is_symlink();

    let canonical = canonicalize_defensively(&absolute_path, path)?;

    let canonical_lexical = classify(&canonical.to_string_lossy());
    if canonical_lexical.is_protected_root() {
        return Err(reject_root(path, &lexical, &canonical_lexical));
    }

    // A dangling symlink's metadata (via symlink_metadata) reports the
    // link itself; `is_dir` should reflect the link's own type for a
    // symlink (never treated as a directory to recurse into), and the
    // real type otherwise.
    let is_dir = if is_symlink {
        false
    } else {
        leaf_meta.is_dir()
    };

    Ok(CanonicalTarget {
        requested: path.to_path_buf(),
        canonical,
        is_symlink,
        is_dir,
    })
}

fn reject_root(
    original: &Path,
    requested_lexical: &LexicalPath,
    resolved_lexical: &LexicalPath,
) -> TargetError {
    let requested_is_literal_root = requested_lexical.is_protected_root();
    if requested_is_literal_root {
        match &resolved_lexical.prefix {
            Prefix::Unc { .. } => TargetError::ShareRoot(original.to_path_buf()),
            _ => TargetError::FilesystemRoot(original.to_path_buf()),
        }
    } else {
        TargetError::ResolvesToRoot {
            requested: original.to_path_buf(),
            resolved: PathBuf::from(resolved_lexical.display_string()),
        }
    }
}

/// Canonicalise `path`, falling back to canonicalising the parent directory
/// and re-joining the final component when the leaf itself is a dangling
/// symlink (whose ultimate target does not exist, so full canonicalisation
/// fails even though the link itself is a perfectly valid delete target).
fn canonicalize_defensively(path: &Path, original: &Path) -> Result<PathBuf, TargetError> {
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(_) => {
            let parent = path.parent().ok_or_else(|| TargetError::Canonicalize {
                path: original.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path has no parent directory",
                ),
            })?;
            let file_name = path.file_name().ok_or_else(|| TargetError::Canonicalize {
                path: original.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path has no final component",
                ),
            })?;
            let canonical_parent =
                std::fs::canonicalize(parent).map_err(|source| TargetError::Canonicalize {
                    path: original.to_path_buf(),
                    source,
                })?;
            Ok(canonical_parent.join(file_name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Layer 1: pure lexical classification -----------------------

    #[test]
    fn rejects_unix_root() {
        assert!(classify("/").is_protected_root());
    }

    #[test]
    fn allows_unix_subpath() {
        assert!(!classify("/var/tmp/cache").is_protected_root());
    }

    #[test]
    fn rejects_windows_drive_root_with_backslash() {
        assert!(classify("C:\\").is_protected_root());
    }

    #[test]
    fn rejects_windows_drive_root_without_trailing_separator() {
        assert!(classify("C:").is_protected_root());
    }

    #[test]
    fn allows_windows_drive_subpath() {
        assert!(!classify("C:\\folder").is_protected_root());
        assert!(!classify("C:\\folder\\file.dat").is_protected_root());
    }

    #[test]
    fn rejects_share_root_with_trailing_backslash() {
        assert!(classify("\\\\server\\share\\").is_protected_root());
    }

    #[test]
    fn rejects_share_root_without_trailing_backslash() {
        assert!(classify("\\\\server\\share").is_protected_root());
    }

    #[test]
    fn allows_share_subpath() {
        assert!(!classify("\\\\server\\share\\folder").is_protected_root());
        assert!(!classify("\\\\server\\share\\folder\\file.dat").is_protected_root());
    }

    #[test]
    fn rejects_dotdot_collapse_to_drive_root() {
        assert!(classify("C:\\folder\\..").is_protected_root());
    }

    #[test]
    fn rejects_dotdot_collapse_to_unix_root() {
        assert!(classify("/var/..").is_protected_root());
        // Sanity check the non-degenerate case is not over-rejected: this
        // resolves to `/var`, one level below root, not to `/` itself.
        assert!(!classify("/var/tmp/..").is_protected_root());
    }

    #[test]
    fn rejects_dotdot_collapse_to_share_root() {
        assert!(classify("\\\\server\\share\\folder\\..").is_protected_root());
    }

    #[test]
    fn allows_dotdot_that_stays_below_root() {
        assert!(!classify("C:\\folder\\sub\\..").is_protected_root());
        // Resolves to C:\folder, still one level below the drive root.
        let p = classify("C:\\folder\\sub\\..");
        assert_eq!(p.components, vec!["folder"]);
    }

    #[test]
    fn handles_verbatim_drive_prefix() {
        assert!(classify("\\\\?\\C:\\").is_protected_root());
        assert!(!classify("\\\\?\\C:\\folder").is_protected_root());
    }

    #[test]
    fn handles_verbatim_unc_prefix() {
        assert!(classify("\\\\?\\UNC\\server\\share").is_protected_root());
        assert!(classify("\\\\?\\UNC\\server\\share\\").is_protected_root());
        assert!(!classify("\\\\?\\UNC\\server\\share\\folder").is_protected_root());
    }

    #[test]
    fn handles_mixed_separators() {
        assert!(classify("C:/folder/..").is_protected_root());
        assert!(!classify("C:/folder/sub").is_protected_root());
    }

    #[test]
    fn empty_path_is_rejected_by_validate_target() {
        let err = validate_target(Path::new("")).unwrap_err();
        assert!(matches!(err, TargetError::EmptyPath));
    }

    #[test]
    fn nonexistent_path_is_not_found() {
        let err = validate_target(Path::new(
            "/this/path/should/not/exist/on/any/machine/xyz123",
        ))
        .unwrap_err();
        assert!(matches!(err, TargetError::NotFound(_)));
    }

    // ---- Layer 2: real filesystem canonicalisation -------------------

    #[test]
    fn allows_real_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let target = validate_target(&child).expect("should be a valid target");
        assert!(!target.is_symlink);
        assert!(target.is_dir);
    }

    #[test]
    fn allows_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, b"hello").unwrap();
        let target = validate_target(&file).expect("should be a valid target");
        assert!(!target.is_dir);
    }

    #[test]
    fn rejects_dotdot_that_escapes_via_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        // child/.. resolves back to `dir`, which is a legitimate non-root
        // target -- this must be *allowed*, proving the resolver does not
        // over-reject ordinary `..` usage that stays inside real, non-root
        // territory.
        let dotdot = child.join("..");
        let target = validate_target(&dotdot).expect("should resolve to a valid non-root target");
        assert_eq!(target.canonical, std::fs::canonicalize(dir.path()).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_that_resolves_to_filesystem_root() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("root_link");
        std::os::unix::fs::symlink("/", &link).unwrap();
        let err = validate_target(&link).unwrap_err();
        assert!(matches!(err, TargetError::ResolvesToRoot { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn allows_symlink_that_resolves_to_ordinary_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let target = validate_target(&link).expect("should be a valid target");
        assert!(target.is_symlink);
    }

    #[cfg(unix)]
    #[test]
    fn allows_dangling_symlink_as_delete_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("does_not_exist"), &link).unwrap();
        let target = validate_target(&link).expect("dangling symlink is still a valid target");
        assert!(target.is_symlink);
        assert!(!target.is_dir);
    }
}
