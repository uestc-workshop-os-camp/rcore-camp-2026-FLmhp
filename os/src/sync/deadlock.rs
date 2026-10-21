//! Per-process deadlock detection for mutexes and semaphores.

use alloc::{vec, vec::Vec};

struct ResourceTracker {
    available: Vec<usize>,
    allocation: Vec<Vec<usize>>,
    need: Vec<Vec<usize>>,
}

impl ResourceTracker {
    fn new() -> Self {
        Self {
            available: Vec::new(),
            allocation: vec![Vec::new()],
            need: vec![Vec::new()],
        }
    }

    fn add_thread(&mut self, tid: usize) {
        let resources = self.available.len();
        if self.allocation.len() <= tid {
            self.allocation.resize(tid + 1, vec![0; resources]);
            self.need.resize(tid + 1, vec![0; resources]);
        }
        self.allocation[tid].fill(0);
        self.need[tid].fill(0);
    }

    fn add_resource(&mut self, id: usize, count: usize) {
        if self.available.len() <= id {
            self.available.resize(id + 1, 0);
            for row in self.allocation.iter_mut() {
                row.resize(id + 1, 0);
            }
            for row in self.need.iter_mut() {
                row.resize(id + 1, 0);
            }
        }
        self.available[id] = count;
    }

    fn request(&mut self, tid: usize, id: usize) {
        self.need[tid][id] += 1;
    }

    fn revoke_request(&mut self, tid: usize, id: usize) {
        debug_assert!(self.need[tid][id] > 0);
        self.need[tid][id] -= 1;
    }

    fn allocate(&mut self, tid: usize, id: usize) {
        self.revoke_request(tid, id);
        debug_assert!(self.available[id] > 0);
        self.available[id] -= 1;
        self.allocation[tid][id] += 1;
    }

    fn release(&mut self, tid: usize, id: usize) -> bool {
        if self.allocation[tid][id] == 0 {
            return false;
        }
        self.allocation[tid][id] -= 1;
        self.available[id] += 1;
        true
    }

    fn release_or_add(&mut self, tid: usize, id: usize) {
        if self.allocation[tid][id] > 0 {
            self.allocation[tid][id] -= 1;
        }
        self.available[id] += 1;
    }

    fn deadlocked(&self) -> bool {
        let mut work = self.available.clone();
        let mut finished = vec![false; self.allocation.len()];
        loop {
            let mut progress = false;
            for tid in 0..self.allocation.len() {
                if !finished[tid]
                    && self.need[tid]
                        .iter()
                        .zip(work.iter())
                        .all(|(need, available)| need <= available)
                {
                    for (available, allocated) in work.iter_mut().zip(self.allocation[tid].iter()) {
                        *available += *allocated;
                    }
                    finished[tid] = true;
                    progress = true;
                }
            }
            if !progress {
                return finished.iter().any(|finished| !finished);
            }
        }
    }
}

/// Tracks resource ownership and rejects requests that make the state unsafe.
pub struct DeadlockDetector {
    enabled: bool,
    mutexes: ResourceTracker,
    semaphores: ResourceTracker,
}

impl DeadlockDetector {
    /// Create a disabled detector with one main thread.
    pub fn new() -> Self {
        Self {
            enabled: false,
            mutexes: ResourceTracker::new(),
            semaphores: ResourceTracker::new(),
        }
    }

    /// Enable or disable request rejection while continuing to track resources.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Add or reset a thread row.
    pub fn add_thread(&mut self, tid: usize) {
        self.mutexes.add_thread(tid);
        self.semaphores.add_thread(tid);
    }

    /// Add a mutex resource.
    pub fn add_mutex(&mut self, id: usize) {
        self.mutexes.add_resource(id, 1);
    }

    /// Add a semaphore resource.
    pub fn add_semaphore(&mut self, id: usize, count: usize) {
        self.semaphores.add_resource(id, count);
    }

    fn request(tracker: &mut ResourceTracker, enabled: bool, tid: usize, id: usize) -> bool {
        tracker.request(tid, id);
        if enabled && tracker.deadlocked() {
            tracker.revoke_request(tid, id);
            false
        } else {
            true
        }
    }

    /// Register a mutex request and report whether it is safe.
    pub fn request_mutex(&mut self, tid: usize, id: usize) -> bool {
        Self::request(&mut self.mutexes, self.enabled, tid, id)
    }

    /// Commit a granted mutex request.
    pub fn allocate_mutex(&mut self, tid: usize, id: usize) {
        self.mutexes.allocate(tid, id);
    }

    /// Record a mutex reacquired inside a condition-variable wait.
    pub fn reacquire_mutex(&mut self, tid: usize, id: usize) {
        debug_assert!(self.mutexes.available[id] > 0);
        self.mutexes.available[id] -= 1;
        self.mutexes.allocation[tid][id] += 1;
    }

    /// Release a mutex held by a thread.
    pub fn release_mutex(&mut self, tid: usize, id: usize) -> bool {
        self.mutexes.release(tid, id)
    }

    /// Register a semaphore request and report whether it is safe.
    pub fn request_semaphore(&mut self, tid: usize, id: usize) -> bool {
        Self::request(&mut self.semaphores, self.enabled, tid, id)
    }

    /// Commit a granted semaphore request.
    pub fn allocate_semaphore(&mut self, tid: usize, id: usize) {
        self.semaphores.allocate(tid, id);
    }

    /// Release one semaphore unit held by a thread.
    pub fn release_semaphore(&mut self, tid: usize, id: usize) {
        self.semaphores.release_or_add(tid, id);
    }
}
