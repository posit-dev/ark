//
// pool.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;

use aether_path::FilePath;
use stdext::panic_message;
use stdext::spawn;

use super::catch_cancellation;
use super::snapshot::WorldStateSnapshot;
use crate::lsp;

/// Enough threads that a handful of open files all get diagnosed in parallel,
/// few enough that they don't crowd out the main loop or the R session we share
/// a process with.
const MAX_ANALYSIS_THREADS: usize = 4;

/// A fixed set of OS threads running analysis tasks over a db snapshot.
///
/// Each task's snapshot is taken at enqueue time on the main loop and sees
/// the state as of that tick. A write waits for those snapshots to drop
/// before it can proceed. This pool doesn't order results across tasks: a
/// diagnostics result carries a generation id and [`DiagnosticsState::accept`]
/// drops staled results.
///
/// A writer never waits on this pool for longer than the one task it
/// interrupted. Queued tasks get thrown away, and the task currently running
/// unwinds at its next salsa query.
pub(crate) struct AnalysisPool {
    shared: Arc<Shared>,
}

impl AnalysisPool {
    pub(crate) fn new() -> Self {
        Self::with_threads(analysis_threads())
    }

    fn with_threads(threads: usize) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                entries: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        });

        for _ in 0..threads {
            let shared = Arc::clone(&shared);
            spawn!("oak-analysis", move || work(shared));
        }

        Self { shared }
    }

    /// Queue `run` behind everything already queued.
    pub(super) fn spawn(
        &self,
        snapshot: WorldStateSnapshot,
        run: impl FnOnce(WorldStateSnapshot) + Send + 'static,
    ) {
        self.push(Entry {
            key: None,
            snapshot,
            run: Box::new(run),
        });
    }

    /// Queue `run`, replacing a queued task with the same `key` that hasn't
    /// started yet. Diagnostics key on the file, so a fresh pass supersedes a
    /// queued predecessor.
    pub(super) fn spawn_keyed(
        &self,
        key: FilePath,
        snapshot: WorldStateSnapshot,
        run: impl FnOnce(WorldStateSnapshot) + Send + 'static,
    ) {
        self.push(Entry {
            key: Some(key),
            snapshot,
            run: Box::new(run),
        });
    }

    fn push(&self, entry: Entry) {
        let mut queue = self.shared.lock();

        if entry.key.is_some() {
            let queued = queue
                .entries
                .iter_mut()
                .find(|queued| queued.key == entry.key);

            // Reuse the slot, so a file that keeps getting edited can't starve
            // the other files behind it.
            if let Some(queued) = queued {
                *queued = entry;
                return;
            }
        }

        queue.entries.push_back(entry);
        drop(queue);
        self.shared.ready.notify_one();
    }
}

/// Analysis tasks are CPU-bound, so don't run more of them than the machine can
/// actually run at once.
fn analysis_threads() -> usize {
    match std::thread::available_parallelism() {
        Ok(parallelism) => parallelism.get().min(MAX_ANALYSIS_THREADS),
        Err(err) => {
            log::warn!("Can't determine available parallelism, using one analysis thread: {err}");
            1
        },
    }
}

/// Closing the queue is all a worker needs to exit, so shutdown doesn't join
/// (and never blocks the caller). Clearing the backlog here releases the db
/// handles those tasks were holding.
impl Drop for AnalysisPool {
    fn drop(&mut self) {
        let mut queue = self.shared.lock();
        queue.closed = true;
        queue.entries.clear();
        drop(queue);
        self.shared.ready.notify_all();
    }
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

struct Queue {
    entries: VecDeque<Entry>,
    closed: bool,
}

struct Entry {
    /// `Some` for a task that a later task with the same key may replace.
    key: Option<FilePath>,
    snapshot: WorldStateSnapshot,
    run: Box<dyn FnOnce(WorldStateSnapshot) + Send>,
}

fn work(shared: Arc<Shared>) {
    // `run_entry` takes the entry by value, so the snapshot has dropped by the
    // time we ask for the next one. A worker parked on `next_entry` doesn't
    // hold a db handle and can't block a writer.
    while let Some(entry) = shared.next_entry() {
        run_entry(entry);
    }
}

impl Shared {
    fn next_entry(&self) -> Option<Entry> {
        let mut queue = self.lock();
        loop {
            if let Some(entry) = queue.entries.pop_front() {
                return Some(entry);
            }
            if queue.closed {
                return None;
            }
            queue = match self.ready.wait(queue) {
                Ok(queue) => queue,
                Err(err) => err.into_inner(),
            };
        }
    }

    /// Tasks never run under this lock, so a poisoned lock still guards a
    /// consistent queue.
    fn lock(&self) -> MutexGuard<'_, Queue> {
        match self.queue.lock() {
            Ok(queue) => queue,
            Err(err) => err.into_inner(),
        }
    }
}

fn run_entry(entry: Entry) {
    let Entry { snapshot, run, .. } = entry;

    // A writer parked on this handle would only cancel the task at its first
    // query, so go straight to dropping the snapshot. This is what lets a
    // backlog drain in one pass while a writer waits.
    if snapshot.is_cancelled() {
        return;
    }

    let task = AssertUnwindSafe(|| catch_cancellation(|| run(snapshot)));
    if let Err(err) = std::panic::catch_unwind(task) {
        lsp::log_error!(
            "An analysis task panicked: {msg}",
            msg = panic_message(err.as_ref())
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use super::AnalysisPool;
    use crate::lsp::state::WorldState;

    /// A queued task whose snapshot is already cancelled must be dropped without
    /// running. That is what lets a backlog release its db handles while a writer
    /// is parked, instead of each task needing a thread first.
    ///
    /// One worker, so the barrier task behind it can only run after the
    /// cancelled task has been dequeued.
    #[test]
    fn test_pool_drops_cancelled_task_without_running() {
        let state = WorldState::default();
        let pool = AnalysisPool::with_threads(1);

        let cancelled = state.snapshot();
        cancelled.cancellation_token().cancel();

        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        pool.spawn(cancelled, move |_snapshot| {
            flag.store(true, Ordering::Release)
        });

        let (barrier_tx, barrier_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            barrier_tx.send(()).unwrap()
        });

        barrier_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(!ran.load(Ordering::Acquire));
    }
}
