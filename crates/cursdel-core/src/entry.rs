//! Types describing a single filesystem object discovered by a platform
//! enumerator, independent of any particular OS.

use std::path::PathBuf;
use std::time::SystemTime;

/// A unique, cheap-to-copy identifier for a directory within one operation,
/// used to track parent/child completion (see `crate::directory_tracker`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DirId(pub u64);

/// Timestamps available for a discovered entry. Platform engines populate
/// whichever of these the underlying filesystem call actually returned;
/// `None` means "not available on this platform/filesystem", not zero.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntryTimes {
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
}

/// What kind of filesystem object was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    /// A real directory that will be recursed into. `DiscoveredEntry::dir_id`
    /// is always `Some` for this kind -- it is the id children will
    /// reference as their `parent_dir_id`.
    Directory,
    /// A symlink/junction/reparse point. CurseDelete deletes the link
    /// object itself and never recurses through it, so this is always a
    /// leaf for directory-completion bookkeeping purposes, exactly like a
    /// `File`. `reparse_is_dir` is the Windows-specific hint that the link
    /// presents as a directory reparse point (junction / directory
    /// symlink), which needs `RemoveDirectoryW`-style removal rather than
    /// `DeleteFileW`-style removal; it is always `false` on POSIX targets,
    /// where `unlink()` handles every symlink uniformly.
    Symlink {
        reparse_is_dir: bool,
    },
    /// Anything else the OS reports (device node, FIFO, socket, ...).
    Other,
}

/// One file, directory, or link discovered while walking a target tree.
#[derive(Debug, Clone)]
pub struct DiscoveredEntry {
    pub path: PathBuf,
    /// Set only when `kind == Directory`: the id this directory was
    /// allocated so its children can reference it as `parent_dir_id`.
    pub dir_id: Option<DirId>,
    pub parent_dir_id: Option<DirId>,
    pub kind: EntryKind,
    pub size: u64,
    pub times: EntryTimes,
    pub readonly: bool,
}

impl DiscoveredEntry {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, EntryKind::Directory)
    }

    /// True for entries the directory-completion tracker treats as leaves:
    /// everything except real directories.
    pub fn is_leaf(&self) -> bool {
        !self.is_dir()
    }
}
