//! The contract every native platform engine (Windows/Linux/macOS)
//! implements. `cursdel-core` owns orchestration -- the bounded streaming
//! pipeline, adaptive worker pool, filters, retention, and reporting --
//! and calls into a `PlatformEngine` only for the operations that are
//! genuinely platform-specific: enumeration, the actual delete syscalls,
//! permission/ACL remediation, and lock handling.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::entry::{DirId, DiscoveredEntry};
use crate::error::DeleteFailure;
use crate::target::CanonicalTarget;

/// Cooperative cancellation flag shared across the enumerator thread and
/// every worker thread. Set on Ctrl+C; checked at safe, frequent points so
/// an interrupted run stops promptly and reports partial results rather
/// than leaving state ambiguous.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Allocates unique [`DirId`]s within one operation. `DirId(0)` is reserved
/// for the operation's root directory and is not produced by this
/// allocator; engines call [`DirIdAllocator::next`] for every subdirectory
/// they discover.
#[derive(Default)]
pub struct DirIdAllocator(AtomicU64);

impl DirIdAllocator {
    pub fn new() -> Self {
        Self(AtomicU64::new(1))
    }

    pub fn next(&self) -> DirId {
        DirId(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

/// Events streamed from a platform enumerator to the pipeline. Enumerators
/// must emit these as entries are discovered rather than buffering the
/// tree -- see `docs/adr/0002-streaming-pipeline.md`.
pub enum EnumEvent {
    Entry(DiscoveredEntry),
    /// Sent exactly once per directory (including the root) once every
    /// child of that directory has been sent (or the directory could not
    /// be listed at all, in which case it is sent immediately with zero
    /// children).
    DirectoryComplete {
        dir_id: DirId,
    },
    /// A non-fatal enumeration error (e.g. a directory could not be
    /// listed). Recorded as a failure; does not stop the rest of the walk.
    Error {
        path: PathBuf,
        message: String,
    },
}

/// The sending half of the enumeration channel, plus the shared directory
/// id allocator, handed to a platform engine's `enumerate` implementation.
pub struct EnumSink {
    tx: crossbeam_channel::Sender<EnumEvent>,
    dir_ids: Arc<DirIdAllocator>,
}

impl EnumSink {
    pub fn new(tx: crossbeam_channel::Sender<EnumEvent>, dir_ids: Arc<DirIdAllocator>) -> Self {
        Self { tx, dir_ids }
    }

    /// Blocks if the bounded channel is full, which is exactly the
    /// backpressure mechanism that keeps memory use bounded regardless of
    /// tree size: a slow delete path throttles a fast enumerator.
    pub fn send(&self, event: EnumEvent) -> Result<(), crossbeam_channel::SendError<EnumEvent>> {
        self.tx.send(event)
    }

    pub fn next_dir_id(&self) -> DirId {
        self.dir_ids.next()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnumError {
    #[error("enumeration was cancelled")]
    Cancelled,
    #[error("enumeration channel closed unexpectedly")]
    ChannelClosed,
}

pub type EnumResult = Result<(), EnumError>;

/// Per-call flags a platform engine needs to decide how aggressively to
/// act. Derived once from [`crate::options::OperationOptions`] and passed
/// by value (it is `Copy`) into every engine call.
#[derive(Debug, Clone, Copy)]
pub struct DeleteOptions {
    pub allow_remediation: bool,
    pub kill_locks: bool,
    pub close_remote_locks: bool,
}

#[derive(Debug, Clone)]
pub enum DeleteOutcome {
    Deleted,
    /// The object was already gone by the time we tried to delete it
    /// (benign race with a concurrent process/enumeration artifact).
    AlreadyGone,
    Failed(DeleteFailure),
}

#[derive(Debug, Clone)]
pub struct DeleteAttempt {
    pub outcome: DeleteOutcome,
    pub bytes: u64,
}

impl DeleteAttempt {
    pub fn deleted(bytes: u64) -> Self {
        Self {
            outcome: DeleteOutcome::Deleted,
            bytes,
        }
    }

    pub fn already_gone() -> Self {
        Self {
            outcome: DeleteOutcome::AlreadyGone,
            bytes: 0,
        }
    }

    pub fn failed(failure: DeleteFailure) -> Self {
        Self {
            outcome: DeleteOutcome::Failed(failure),
            bytes: 0,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(
            self.outcome,
            DeleteOutcome::Deleted | DeleteOutcome::AlreadyGone
        )
    }
}

#[derive(Debug, Clone)]
pub enum RemediationOutcome {
    /// Remediation was applied; the caller should retry the delete once.
    Applied,
    /// This failure category is not something remediation can address.
    NotApplicable,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum LockResolution {
    Resolved,
    /// This platform/target does not support this kind of lock handling at
    /// all (e.g. `--close-remote-locks` on a non-Windows-server share).
    /// Never reported as success.
    Unsupported(String),
    Failed(String),
}

/// The native engine contract. Implementations live in `cursdel-windows`,
/// `cursdel-linux`, and `cursdel-macos`.
pub trait PlatformEngine: Send + Sync {
    fn name(&self) -> &'static str;

    /// Stream every descendant of `root` to `sink`. Must return promptly
    /// after `cancel` is set. Must not buffer the whole tree in memory.
    fn enumerate(
        &self,
        root: &CanonicalTarget,
        sink: &EnumSink,
        cancel: &CancelToken,
    ) -> EnumResult;

    fn delete_file(&self, path: &Path, opts: DeleteOptions) -> DeleteAttempt;

    /// Remove an already-empty directory.
    fn delete_dir(&self, path: &Path, opts: DeleteOptions) -> DeleteAttempt;

    /// Attempt to remediate a failed delete (attributes/ownership/ACLs).
    /// Only invoked when `opts.allow_remediation` is true and the failure
    /// category is potentially remediable.
    fn remediate(
        &self,
        path: &Path,
        is_dir: bool,
        failure: &DeleteFailure,
        opts: DeleteOptions,
    ) -> RemediationOutcome;

    /// `--kill-locks`: identify and resolve a local process holding `path`
    /// open. Platforms with no meaningful local-lock story return
    /// `Unsupported`.
    fn resolve_local_lock(&self, path: &Path, opts: DeleteOptions) -> LockResolution;

    /// `--close-remote-locks`: administratively close a remote SMB open on
    /// a supported Windows file server. Non-Windows engines always return
    /// `Unsupported`.
    fn resolve_remote_lock(&self, path: &Path, opts: DeleteOptions) -> LockResolution;
}
