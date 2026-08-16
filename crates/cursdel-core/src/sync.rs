//! A small resizable counting semaphore used to gate how many pre-spawned
//! worker threads may be *actively* processing work at once.
//!
//! CurseDelete pre-spawns up to the configured worker ceiling as ordinary
//! OS threads once (cheap: a blocked thread costs a kernel stack, not
//! CPU), and uses this semaphore to control how many of them are allowed
//! to run concurrently at any moment. Growing/shrinking concurrency is
//! then just changing a number, not spawning or joining OS threads at
//! runtime -- see `docs/adr/0003-adaptive-workers.md`.

use std::sync::{Condvar, Mutex};

struct State {
    granted: usize,
    capacity: usize,
}

pub struct ResizableSemaphore {
    state: Mutex<State>,
    cv: Condvar,
}

impl ResizableSemaphore {
    pub fn new(initial_capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                granted: 0,
                capacity: initial_capacity.max(1),
            }),
            cv: Condvar::new(),
        }
    }

    /// Block until a permit is available, then take it.
    pub fn acquire(&self) {
        let mut state = self.state.lock().unwrap();
        while state.granted >= state.capacity {
            state = self.cv.wait(state).unwrap();
        }
        state.granted += 1;
    }

    pub fn release(&self) {
        let mut state = self.state.lock().unwrap();
        state.granted = state.granted.saturating_sub(1);
        drop(state);
        self.cv.notify_one();
    }

    /// Change the target concurrency. Threads already holding a permit are
    /// never forcibly revoked; a shrink simply blocks future acquires
    /// sooner.
    pub fn set_capacity(&self, new_capacity: usize) {
        let mut state = self.state.lock().unwrap();
        state.capacity = new_capacity.max(1);
        drop(state);
        self.cv.notify_all();
    }

    pub fn capacity(&self) -> usize {
        self.state.lock().unwrap().capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn respects_capacity_under_contention() {
        let sem = Arc::new(ResizableSemaphore::new(3));
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|scope| {
            for _ in 0..20 {
                let sem = sem.clone();
                let concurrent = concurrent.clone();
                let max_seen = max_seen.clone();
                scope.spawn(move || {
                    sem.acquire();
                    let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                    sem.release();
                });
            }
        });

        assert!(max_seen.load(Ordering::SeqCst) <= 3);
    }

    #[test]
    fn growing_capacity_admits_more_waiters() {
        let sem = Arc::new(ResizableSemaphore::new(1));
        sem.acquire(); // hold the only permit
        sem.set_capacity(2);
        // A second acquire should now succeed without needing the first
        // permit to be released.
        let sem2 = sem.clone();
        let handle = std::thread::spawn(move || sem2.acquire());
        handle.join().unwrap();
    }
}
