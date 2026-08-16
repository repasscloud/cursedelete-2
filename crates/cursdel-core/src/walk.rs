//! A generic, streaming, cancellation-safe tree walker shared by every
//! platform engine.
//!
//! The tricky part of enumeration -- allocating [`DirId`]s, emitting
//! entries and `DirectoryComplete` events in a way `crate::directory_tracker`
//! can always eventually resolve (even under cancellation), and doing it
//! with an explicit stack rather than OS-thread recursion so arbitrarily
//! deep trees can't blow the stack -- is not platform-specific. Only the
//! call that lists one directory's immediate children is. Each platform
//! engine therefore implements [`DirLister`] with its fastest native
//! primitive (`FindFirstFileExW`, `readdir`/`getdents64`, ...) and calls
//! [`stream_tree`] to do the rest.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::engine::{CancelToken, EnumError, EnumEvent, EnumResult, EnumSink};
use crate::entry::{DirId, DiscoveredEntry, EntryKind, EntryTimes};
use crate::target::CanonicalTarget;

/// One immediate child of a directory, as reported by a platform's native
/// listing primitive.
pub struct RawChild {
    pub name: OsString,
    pub is_dir: bool,
    /// A reparse point (Windows junction/symlink) or POSIX symlink. Never
    /// recursed into -- deleted as a leaf, matching the product's
    /// symlink/reparse safety rule.
    pub is_reparse_point: bool,
    pub size: u64,
    pub times: EntryTimes,
    pub readonly: bool,
}

/// Lists the immediate children of one directory using the platform's
/// fastest native primitive.
pub trait DirLister: Sync {
    fn list_children(&self, dir: &Path) -> std::io::Result<Vec<RawChild>>;
}

struct OpenDir {
    dir_id: DirId,
    path: PathBuf,
}

/// Streams every descendant of `root` to `sink`, in the shape
/// `crate::directory_tracker::DirectoryTracker` expects. The caller is
/// responsible for registering `DirId(0)` for `root` itself before calling
/// this (see `crate::pipeline`) -- this function only ever emits entries
/// and completions for `root`'s descendants.
///
/// On cancellation, every directory still open on the traversal stack is
/// flushed with a `DirectoryComplete` event before returning
/// `Err(EnumError::Cancelled)`, so the directory tracker can always reach a
/// consistent terminal state and the pipeline never hangs waiting for a
/// directory that will never finish enumerating.
pub fn stream_tree(
    root: &CanonicalTarget,
    lister: &dyn DirLister,
    sink: &EnumSink,
    cancel: &CancelToken,
) -> EnumResult {
    let mut stack: Vec<OpenDir> = vec![OpenDir {
        dir_id: DirId(0),
        path: root.canonical.clone(),
    }];

    let mut cancelled = false;

    while let Some(current) = stack.pop() {
        if cancel.is_cancelled() {
            cancelled = true;
            // Put it back so the flush loop below handles it uniformly.
            stack.push(current);
            break;
        }

        match lister.list_children(&current.path) {
            Ok(children) => {
                for child in children {
                    let child_path = current.path.join(&child.name);

                    if child.is_dir && !child.is_reparse_point {
                        let child_dir_id = sink.next_dir_id();
                        send(
                            sink,
                            EnumEvent::Entry(DiscoveredEntry {
                                path: child_path.clone(),
                                dir_id: Some(child_dir_id),
                                parent_dir_id: Some(current.dir_id),
                                kind: EntryKind::Directory,
                                size: 0,
                                times: child.times,
                                readonly: child.readonly,
                            }),
                        )?;
                        stack.push(OpenDir {
                            dir_id: child_dir_id,
                            path: child_path,
                        });
                    } else {
                        let kind = if child.is_reparse_point {
                            EntryKind::Symlink {
                                reparse_is_dir: child.is_dir,
                            }
                        } else {
                            EntryKind::File
                        };
                        send(
                            sink,
                            EnumEvent::Entry(DiscoveredEntry {
                                path: child_path,
                                dir_id: None,
                                parent_dir_id: Some(current.dir_id),
                                kind,
                                size: child.size,
                                times: child.times,
                                readonly: child.readonly,
                            }),
                        )?;
                    }
                }
            }
            Err(e) => {
                send(
                    sink,
                    EnumEvent::Error {
                        path: current.path.clone(),
                        message: e.to_string(),
                    },
                )?;
            }
        }

        send(
            sink,
            EnumEvent::DirectoryComplete {
                dir_id: current.dir_id,
            },
        )?;
    }

    if cancelled {
        // Flush every directory still open on the stack (including the one
        // we were about to process) so the tracker can still reach a
        // consistent terminal state.
        for open in stack {
            send(
                sink,
                EnumEvent::DirectoryComplete {
                    dir_id: open.dir_id,
                },
            )?;
        }
        return Err(EnumError::Cancelled);
    }

    Ok(())
}

fn send(sink: &EnumSink, event: EnumEvent) -> EnumResult {
    sink.send(event).map_err(|_| EnumError::ChannelClosed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DirIdAllocator;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// An in-memory directory tree used to test the walker without
    /// touching a real filesystem or depending on any platform engine.
    struct FakeFs {
        children: HashMap<PathBuf, Vec<RawChild>>,
    }

    impl DirLister for FakeFs {
        fn list_children(&self, dir: &Path) -> std::io::Result<Vec<RawChild>> {
            match self.children.get(dir) {
                Some(v) => Ok(v
                    .iter()
                    .map(|c| RawChild {
                        name: c.name.clone(),
                        is_dir: c.is_dir,
                        is_reparse_point: c.is_reparse_point,
                        size: c.size,
                        times: c.times,
                        readonly: c.readonly,
                    })
                    .collect()),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated listing failure",
                )),
            }
        }
    }

    fn file(name: &str) -> RawChild {
        RawChild {
            name: name.into(),
            is_dir: false,
            is_reparse_point: false,
            size: 10,
            times: EntryTimes::default(),
            readonly: false,
        }
    }

    fn dir(name: &str) -> RawChild {
        RawChild {
            name: name.into(),
            is_dir: true,
            is_reparse_point: false,
            size: 0,
            times: EntryTimes::default(),
            readonly: false,
        }
    }

    fn dir_symlink(name: &str) -> RawChild {
        RawChild {
            name: name.into(),
            is_dir: true,
            is_reparse_point: true,
            size: 0,
            times: EntryTimes::default(),
            readonly: false,
        }
    }

    fn drain_events(rx: crossbeam_channel::Receiver<EnumEvent>) -> Vec<EnumEvent> {
        rx.try_iter().collect()
    }

    fn fake_root() -> CanonicalTarget {
        CanonicalTarget {
            requested: PathBuf::from("/root"),
            canonical: PathBuf::from("/root"),
            is_symlink: false,
            is_dir: true,
        }
    }

    #[test]
    fn walks_nested_tree_and_completes_every_directory() {
        let mut children = HashMap::new();
        children.insert(PathBuf::from("/root"), vec![file("a.txt"), dir("sub")]);
        children.insert(PathBuf::from("/root/sub"), vec![file("b.txt")]);
        let fs = FakeFs { children };

        let (tx, rx) = crossbeam_channel::unbounded();
        let sink = EnumSink::new(tx, Arc::new(DirIdAllocator::new()));
        let cancel = CancelToken::new();

        stream_tree(&fake_root(), &fs, &sink, &cancel).expect("should succeed");
        drop(sink);

        let events = drain_events(rx);
        let entry_count = events
            .iter()
            .filter(|e| matches!(e, EnumEvent::Entry(_)))
            .count();
        let complete_count = events
            .iter()
            .filter(|e| matches!(e, EnumEvent::DirectoryComplete { .. }))
            .count();

        assert_eq!(entry_count, 3); // a.txt, sub, b.txt
        assert_eq!(complete_count, 2); // /root and /root/sub
    }

    #[test]
    fn does_not_recurse_into_directory_symlinks() {
        let mut children = HashMap::new();
        children.insert(PathBuf::from("/root"), vec![dir_symlink("junction")]);
        let fs = FakeFs { children };

        let (tx, rx) = crossbeam_channel::unbounded();
        let sink = EnumSink::new(tx, Arc::new(DirIdAllocator::new()));
        let cancel = CancelToken::new();

        stream_tree(&fake_root(), &fs, &sink, &cancel).expect("should succeed");
        drop(sink);

        let events = drain_events(rx);
        // The junction is reported as a leaf Symlink entry; since it was
        // never pushed onto the walk stack, no listing of its target ever
        // happens (the FakeFs would return an Err/panic-free empty result
        // only if asked, and it is never asked).
        let symlink_entries: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EnumEvent::Entry(entry) => Some(entry),
                _ => None,
            })
            .collect();
        assert_eq!(symlink_entries.len(), 1);
        assert!(matches!(
            symlink_entries[0].kind,
            EntryKind::Symlink {
                reparse_is_dir: true
            }
        ));
    }

    #[test]
    fn listing_failure_still_completes_the_directory() {
        // No entry for "/root" in the map -> list_children returns Err.
        let fs = FakeFs {
            children: HashMap::new(),
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let sink = EnumSink::new(tx, Arc::new(DirIdAllocator::new()));
        let cancel = CancelToken::new();

        stream_tree(&fake_root(), &fs, &sink, &cancel).expect("should still succeed overall");
        drop(sink);

        let events = drain_events(rx);
        assert!(events.iter().any(|e| matches!(e, EnumEvent::Error { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, EnumEvent::DirectoryComplete { dir_id } if *dir_id == DirId(0))));
    }

    #[test]
    fn cancellation_flushes_remaining_open_directories() {
        let mut children = HashMap::new();
        // A wide root so there is still an unlisted directory left on the
        // stack when we cancel after the first pop.
        children.insert(PathBuf::from("/root"), vec![dir("a"), dir("b"), dir("c")]);
        children.insert(PathBuf::from("/root/a"), vec![]);
        children.insert(PathBuf::from("/root/b"), vec![]);
        children.insert(PathBuf::from("/root/c"), vec![]);
        let fs = FakeFs { children };

        let (tx, rx) = crossbeam_channel::unbounded();
        let sink = EnumSink::new(tx, Arc::new(DirIdAllocator::new()));
        let cancel = CancelToken::new();
        cancel.cancel(); // cancel before the walk even starts

        let result = stream_tree(&fake_root(), &fs, &sink, &cancel);
        drop(sink);
        assert!(matches!(result, Err(EnumError::Cancelled)));

        let events = drain_events(rx);
        // Root itself must still be completed even though nothing was
        // ever listed, so the tracker can resolve it.
        assert!(events
            .iter()
            .any(|e| matches!(e, EnumEvent::DirectoryComplete { dir_id } if *dir_id == DirId(0))));
    }
}
