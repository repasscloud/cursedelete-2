//! Tracks when a directory becomes eligible for its own deletion: only once
//! its enumeration is complete *and* every child (file, symlink, or nested
//! directory) discovered under it has been fully resolved.
//!
//! This is the mechanism behind "directories are deleted only after
//! relevant child processing is complete" (see
//! `docs/adr/0002-streaming-pipeline.md`). Memory use here is bounded by
//! the number of directories *in flight* at any moment, not by total file
//! count -- a file never occupies an entry in this map, only a decrement
//! against its parent's pending count.
//!
//! [`DirectoryTracker`] is accessed concurrently from many worker threads
//! (every completed delete decrements a parent's pending count) so it is
//! built on a sharded concurrent map rather than a single global lock,
//! which would otherwise become exactly the throughput bottleneck this
//! product exists to avoid.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use dashmap::DashMap;

use crate::entry::DirId;

struct DirState {
    parent: Option<DirId>,
    path: PathBuf,
    pending: i64,
    /// Count of children that are still physically present -- either
    /// retained by a filter or left behind by a failed delete. Used only
    /// to *simulate* directory emptiness for `--dry-run`, which never
    /// calls a real `delete_dir`; real operations instead trust the
    /// filesystem's own answer when they attempt the delete (see
    /// `crate::pipeline`).
    still_present: i64,
    enumeration_done: bool,
    /// Once true, this entry has been handed off for finalisation exactly
    /// once. Guards against a torn race where a concurrent
    /// `mark_enumeration_done` and `complete_child` both observe
    /// `pending == 0` and both try to finalise the same directory.
    finalised: bool,
}

/// A directory that has become ready for its own delete attempt: every
/// child has resolved and enumeration of the directory is complete.
pub struct ReadyDirectory {
    pub dir_id: DirId,
    pub parent: Option<DirId>,
    pub path: PathBuf,
    /// Dry-run-only simulation of whether this directory would still have
    /// content left in it (see `still_present` above). Real operations
    /// must not use this to decide whether to attempt deletion -- they
    /// attempt it and read the filesystem's answer.
    pub simulated_non_empty: bool,
}

#[derive(Clone)]
pub struct DirectoryTracker {
    state: Arc<DashMap<DirId, DirState>>,
    ready_tx: Sender<ReadyDirectory>,
}

impl DirectoryTracker {
    /// `ready_tx` receives directories as they become eligible for
    /// deletion; a small dedicated worker pool drains it (see
    /// `crate::pipeline`), keeping directory removal off the hot file
    /// path.
    pub fn new(ready_tx: Sender<ReadyDirectory>) -> Self {
        Self {
            state: Arc::new(DashMap::new()),
            ready_tx,
        }
    }

    /// Register a newly discovered directory (including the root, which
    /// the pipeline registers with `parent = None` before enumeration
    /// starts).
    pub fn register_directory(&self, dir_id: DirId, parent: Option<DirId>, path: PathBuf) {
        self.state.insert(
            dir_id,
            DirState {
                parent,
                path,
                pending: 0,
                still_present: 0,
                enumeration_done: false,
                finalised: false,
            },
        );
        if let Some(parent_id) = parent {
            self.add_pending(parent_id, 1);
        }
    }

    /// A leaf entry (file or symlink) was discovered under `parent`.
    pub fn register_leaf_child(&self, parent: DirId) {
        self.add_pending(parent, 1);
    }

    fn add_pending(&self, dir_id: DirId, delta: i64) {
        if let Some(mut state) = self.state.get_mut(&dir_id) {
            state.pending += delta;
        }
    }

    /// The enumerator finished listing `dir_id` (including the degenerate
    /// case where listing failed and zero children were discovered).
    pub fn mark_enumeration_done(&self, dir_id: DirId) {
        let ready = {
            if let Some(mut state) = self.state.get_mut(&dir_id) {
                state.enumeration_done = true;
                state.pending == 0 && !state.finalised
            } else {
                false
            }
        };
        if ready {
            self.try_finalise(dir_id);
        }
    }

    /// A child of `parent` (file, symlink, or a nested directory that has
    /// itself just been finalised) has fully resolved. `still_present`
    /// means the child was retained by a filter, left behind by a failed
    /// delete, or (for a nested directory) itself simulated as non-empty.
    pub fn complete_child(&self, parent: DirId, still_present: bool) {
        let ready = {
            if let Some(mut state) = self.state.get_mut(&parent) {
                state.pending -= 1;
                if still_present {
                    state.still_present += 1;
                }
                debug_assert!(
                    state.pending >= 0,
                    "directory tracker pending count went negative for {:?}",
                    parent
                );
                state.enumeration_done && state.pending <= 0 && !state.finalised
            } else {
                false
            }
        };
        if ready {
            self.try_finalise(parent);
        }
    }

    fn try_finalise(&self, dir_id: DirId) {
        let ready_directory = {
            let mut entry = match self.state.get_mut(&dir_id) {
                Some(e) => e,
                None => return,
            };
            if entry.finalised {
                return;
            }
            entry.finalised = true;
            ReadyDirectory {
                dir_id,
                parent: entry.parent,
                path: entry.path.clone(),
                simulated_non_empty: entry.still_present > 0,
            }
        };
        // Send outside the map borrow to avoid holding a shard lock while
        // potentially blocking on a full bounded channel.
        let _ = self.ready_tx.send(ready_directory);
    }

    /// Remove bookkeeping for a directory once its own delete attempt has
    /// been resolved and (if it has a parent) that completion has been
    /// propagated. Keeps the map bounded to in-flight directories.
    pub fn forget(&self, dir_id: DirId) {
        self.state.remove(&dir_id);
    }

    /// Number of directories currently tracked (in flight). Exposed for
    /// tests and diagnostics -- this is the bound that keeps memory use
    /// independent of total file count.
    pub fn in_flight_directories(&self) -> usize {
        self.state.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (
        DirectoryTracker,
        crossbeam_channel::Receiver<ReadyDirectory>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (DirectoryTracker::new(tx), rx)
    }

    #[test]
    fn empty_directory_becomes_ready_immediately_on_enumeration_done() {
        let (tracker, rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.mark_enumeration_done(DirId(0));
        let ready = rx.try_recv().expect("root should be ready");
        assert_eq!(ready.dir_id, DirId(0));
    }

    #[test]
    fn directory_waits_for_all_children_before_becoming_ready() {
        let (tracker, rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.register_leaf_child(DirId(0));
        tracker.register_leaf_child(DirId(0));
        tracker.mark_enumeration_done(DirId(0));

        // Not ready yet: two children still pending.
        assert!(rx.try_recv().is_err());

        tracker.complete_child(DirId(0), false);
        assert!(rx.try_recv().is_err());

        tracker.complete_child(DirId(0), false);
        let ready = rx.try_recv().expect("root should now be ready");
        assert_eq!(ready.dir_id, DirId(0));
        assert!(!ready.simulated_non_empty);
    }

    #[test]
    fn retained_child_marks_directory_as_simulated_non_empty() {
        let (tracker, rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.register_leaf_child(DirId(0));
        tracker.complete_child(DirId(0), true); // retained by a filter
        tracker.mark_enumeration_done(DirId(0));
        let ready = rx.try_recv().expect("root should be ready");
        assert!(ready.simulated_non_empty);
    }

    #[test]
    fn order_of_enumeration_done_and_last_child_does_not_matter() {
        // Case A: last child completes before enumeration_done is marked.
        let (tracker, rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.register_leaf_child(DirId(0));
        tracker.complete_child(DirId(0), false);
        tracker.mark_enumeration_done(DirId(0));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn nested_directory_propagates_completion_to_parent() {
        let (tracker, rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.register_directory(DirId(1), Some(DirId(0)), PathBuf::from("/root/sub"));
        tracker.mark_enumeration_done(DirId(1)); // empty subdirectory
        let ready = rx.try_recv().expect("subdirectory should be ready");
        assert_eq!(ready.dir_id, DirId(1));
        assert_eq!(ready.parent, Some(DirId(0)));

        // Root not ready yet -- it still has the subdirectory as a pending child
        // until the pipeline explicitly reports completion via complete_child.
        assert!(rx.try_recv().is_err());

        tracker.forget(DirId(1));
        tracker.complete_child(DirId(0), false);
        tracker.mark_enumeration_done(DirId(0));
        let ready = rx.try_recv().expect("root should now be ready");
        assert_eq!(ready.dir_id, DirId(0));
    }

    #[test]
    fn finalisation_happens_exactly_once_under_racing_calls() {
        let (tracker, rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.register_leaf_child(DirId(0));
        // Simulate the two ways to reach pending==0 racing: mark done first,
        // completing the child second, then call mark_enumeration_done a
        // second time defensively -- must still only fire once.
        tracker.mark_enumeration_done(DirId(0));
        tracker.complete_child(DirId(0), false);
        tracker.mark_enumeration_done(DirId(0));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn in_flight_bound_shrinks_after_forget() {
        let (tracker, _rx) = setup();
        tracker.register_directory(DirId(0), None, PathBuf::from("/root"));
        tracker.register_directory(DirId(1), Some(DirId(0)), PathBuf::from("/root/sub"));
        assert_eq!(tracker.in_flight_directories(), 2);
        tracker.forget(DirId(1));
        assert_eq!(tracker.in_flight_directories(), 1);
    }
}
