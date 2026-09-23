//
// pool.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;

use aether_path::FilePath;
use stdext::spawn;
use tokio::sync::Notify;

use super::catch_cancellation;
use super::snapshot::WorldStateSnapshot;
use crate::lsp;
use crate::lsp::main_loop::LspServiceContext;
use crate::panic;
use crate::panic::Recovery;

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
    pub(crate) fn new(service_context: Arc<LspServiceContext>) -> Self {
        Self::with_threads(analysis_threads(), service_context)
    }

    fn with_threads(threads: usize, service_context: Arc<LspServiceContext>) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                entries: VecDeque::new(),
                active: 0,
                closed: false,
                metrics: PoolMetrics::default(),
            }),
            ready: Condvar::new(),
            idle: Arc::new(Notify::new()),
        });

        for _ in 0..threads {
            let shared = Arc::clone(&shared);
            let service_context = Arc::clone(&service_context);
            spawn!("oak-analysis", move || {
                // `run_entry()` recovers task panics. A panic that reaches this
                // boundary comes from the worker loop itself, so let this
                // worker exit rather than abort the process. The remaining
                // workers keep draining the queue.
                let outcome =
                    panic::catch_unwind(Recovery::Always, || work(&shared, &service_context));
                if let Err(message) = outcome {
                    lsp::log_error!("Panic in an analysis worker: {message}");
                    service_context.report_background_panic();
                }
            });
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
        queue.metrics.queued += 1;

        if entry.key.is_some() {
            let queued = queue
                .entries
                .iter_mut()
                .find(|queued| queued.key == entry.key);

            // Reuse the slot, so a file that keeps getting edited can't starve
            // the other files behind it.
            if let Some(queued) = queued {
                *queued = entry;
                queue.metrics.replaced += 1;
                return;
            }
        }

        queue.entries.push_back(entry);

        let len = queue.entries.len();
        let metrics = &mut queue.metrics;
        metrics.peak_queue_len = metrics.peak_queue_len.max(len);

        drop(queue);
        self.shared.ready.notify_one();
    }

    pub(crate) fn metrics(&self) -> PoolMetrics {
        self.shared.lock().metrics
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.shared.lock().is_idle()
    }

    /// Wait for an idle transition, then recheck the queue state.
    ///
    /// [`Notify::notify_one()`] retains one permit across the race between
    /// checking the queue and waiting. The permit does not guarantee that the
    /// pool is still idle, so callers must recheck the queue after waking.
    #[cfg(test)]
    pub(crate) async fn idle(&self) {
        self.shared.idle.notified().await
    }

    #[cfg(test)]
    pub(crate) fn spawn_test_task(
        &self,
        snapshot: WorldStateSnapshot,
        task: impl FnOnce(WorldStateSnapshot) + Send + 'static,
    ) {
        self.spawn(snapshot, task)
    }

    /// Return an owned signal so callers can wait while mutably borrowing the
    /// state that owns this pool.
    #[cfg(test)]
    pub(crate) fn idle_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.shared.idle)
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

    /// Workers can signal an idle transition without entering a Tokio runtime.
    /// The waiter retains the `Arc` while mutably borrowing the owning state.
    idle: Arc<Notify>,
}

/// Counters describing how the analysis queue processes tasks.
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct PoolMetrics {
    /// Submitted tasks, including replacements.
    pub(crate) queued: u64,
    /// Queued entries replaced by a newer task with the same key.
    pub(crate) replaced: u64,
    pub(crate) started: u64,
    pub(crate) completed: u64,
    /// Tasks cancelled before a worker started them.
    pub(crate) cancelled_queued: u64,
    /// Tasks cancelled after a worker started them.
    pub(crate) cancelled_running: u64,
    /// Tasks whose panic a worker caught.
    pub(crate) panicked: u64,
    pub(crate) peak_queue_len: usize,
}

impl PoolMetrics {
    /// Entries still queued. Derived by subtracting every outcome that removes
    /// an entry. A negative value signals unbalanced counters.
    pub(crate) fn waiting(&self) -> i64 {
        self.queued as i64 -
            self.replaced as i64 -
            self.cancelled_queued as i64 -
            self.started as i64
    }

    /// Tasks still running in workers. Derived by subtracting every terminal
    /// outcome after a task starts. A negative value signals unbalanced counters.
    pub(crate) fn running(&self) -> i64 {
        self.started as i64 -
            self.completed as i64 -
            self.cancelled_running as i64 -
            self.panicked as i64
    }
}

struct Queue {
    entries: VecDeque<Entry>,
    /// Entries owned by workers, including cancelled snapshots being discarded.
    active: usize,
    closed: bool,
    metrics: PoolMetrics,
}

impl Queue {
    fn is_idle(&self) -> bool {
        self.entries.is_empty() && self.active == 0
    }
}

struct Entry {
    /// `Some` for a task that a later task with the same key may replace.
    key: Option<FilePath>,
    snapshot: WorldStateSnapshot,
    run: Box<dyn FnOnce(WorldStateSnapshot) + Send>,
}

fn work(shared: &Shared, service_context: &LspServiceContext) {
    // `run_entry` takes the entry by value, so the snapshot has dropped by the
    // time we ask for the next one. A worker parked on `next_entry` doesn't
    // hold a db handle and can't block a writer.
    while let Some(entry) = shared.next_entry() {
        run_entry(entry, shared, service_context);
    }
}

impl Shared {
    fn next_entry(&self) -> Option<Entry> {
        let mut queue = self.lock();
        loop {
            if let Some(entry) = queue.entries.pop_front() {
                queue.active += 1;
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

fn run_entry(entry: Entry, shared: &Shared, service_context: &LspServiceContext) {
    let Entry { snapshot, run, .. } = entry;

    // A writer parked on this handle would only cancel the task at its first
    // query, so go straight to dropping the snapshot. This is what lets a
    // backlog drain in one pass while a writer waits.
    if snapshot.is_cancelled() {
        finish_entry(shared, |metrics| metrics.cancelled_queued += 1);
        return;
    }

    record_started(shared);

    match panic::catch_unwind(Recovery::Always, || catch_cancellation(|| run(snapshot))) {
        Ok(Some(())) => {
            finish_entry(shared, |metrics| metrics.completed += 1);
        },
        Ok(None) => {
            finish_entry(shared, |metrics| metrics.cancelled_running += 1);
        },
        Err(message) => {
            finish_entry(shared, |metrics| metrics.panicked += 1);
            lsp::log_error!("An analysis task panicked: {message}");
            service_context.report_background_panic();
        },
    }
}

fn record_started(shared: &Shared) {
    let metrics = {
        let mut queue = shared.lock();
        queue.metrics.started += 1;
        queue.metrics
    };
    lsp::log_trace!("Analysis queue: {metrics:?}");
}

/// Record a task's terminal outcome and release the worker's ownership of it.
/// Cancellation and panic use the same path, so a task that emits no main-loop
/// event can still wake the harness when the pool becomes idle.
fn finish_entry(shared: &Shared, record_outcome: impl FnOnce(&mut PoolMetrics)) {
    let (metrics, idle) = {
        let mut queue = shared.lock();

        assert!(queue.active > 0);
        queue.active -= 1;
        record_outcome(&mut queue.metrics);

        (queue.metrics, queue.is_idle())
    };

    if idle {
        shared.idle.notify_one();
    }

    lsp::log_trace!("Analysis queue: {metrics:?}");
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use aether_path::FilePath;
    use url::Url;

    use super::AnalysisPool;
    use crate::lsp::main_loop::LspServiceContext;
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
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

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

        // With one worker, receiving the barrier signal proves the first task
        // updated `cancelled_queued`. Completion and panic counters can still
        // race with this thread, so this test does not assert on them.
        let counts = pool.metrics();
        assert_eq!(counts.queued, 2);
        assert_eq!(counts.cancelled_queued, 1);
        assert_eq!(counts.started, 1);
        assert_eq!(counts.waiting(), 0);
    }

    /// Install the production hook so a missing `catch_unwind()` aborts the
    /// process instead of silently losing the worker panic.
    #[test]
    fn test_pool_survives_panicking_task() {
        crate::panic::install();

        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

        pool.spawn(state.snapshot(), |_snapshot| {
            panic!("Test panic in an analysis task")
        });

        let (barrier_tx, barrier_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            barrier_tx.send(()).unwrap()
        });

        barrier_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();

        let counts = pool.metrics();
        assert_eq!(counts.started, 2);
        assert_eq!(counts.panicked, 1);
    }

    /// Panics when dropped, so a task panicking with it as payload makes
    /// `run_entry()` panic again when it drops the payload after its own
    /// `catch_unwind()` returns.
    struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("Test panic while dropping a task's panic payload");
        }
    }

    /// Install the production hook so a panic that escapes the worker's
    /// boundary aborts the test process. Two workers, so the one left after
    /// the other exits must still run tasks.
    #[test]
    fn test_pool_survives_panic_outside_a_task() {
        crate::panic::install();

        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(2, Arc::clone(&context));
        pool.spawn(state.snapshot(), |_snapshot| {
            std::panic::panic_any(PanicOnDrop)
        });

        // Each worker holds a clone of `context` and drops it when its thread
        // returns through the boundary. An escaped panic aborts before that.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while Arc::strong_count(&context) > 2 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let (ran_tx, ran_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| ran_tx.send(()).unwrap());
        ran_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    }

    /// Cancellation after a task starts increments `cancelled_running`, rather
    /// than `cancelled_queued`.
    #[test]
    fn test_pool_records_cancelled_running_task() {
        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

        pool.spawn(state.snapshot(), |snapshot| {
            snapshot.cancellation_token().cancel();
            salsa::Database::unwind_if_revision_cancelled(snapshot.db());
        });

        let (barrier_tx, barrier_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            barrier_tx.send(()).unwrap()
        });

        barrier_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();

        let counts = pool.metrics();
        assert_eq!(counts.started, 2);
        assert_eq!(counts.cancelled_running, 1);
    }

    #[test]
    fn test_pool_records_completed_task() {
        crate::panic::install();

        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

        pool.spawn(state.snapshot(), |_snapshot| {});

        // The barrier panics after signalling, so it cannot increment
        // `completed` before this thread reads the counters. A normal barrier
        // could race because completion is recorded only after its closure returns.
        let (barrier_tx, barrier_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            barrier_tx.send(()).unwrap();
            panic!("Barrier task panics so it never counts as `completed`");
        });

        barrier_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();

        let counts = pool.metrics();
        assert_eq!(counts.queued, 2);
        assert_eq!(counts.started, 2);
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.waiting(), 0);
    }

    /// A second keyed task queued behind the first, before either has started,
    /// replaces it in place instead of queuing separately.
    ///
    /// The worker is parked on a first, unkeyed task for the whole setup, so
    /// both keyed pushes (and the replacement) happen on this thread before
    /// the worker can dequeue anything.
    #[test]
    fn test_pool_records_keyed_replacement() {
        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            release_rx.recv().unwrap();
        });

        let key = FilePath::from_url(&Url::parse("file:///test.R").unwrap());

        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        pool.spawn_keyed(key.clone(), state.snapshot(), move |_snapshot| {
            flag.store(true, Ordering::Release);
        });

        let (barrier_tx, barrier_rx) = std::sync::mpsc::channel();
        pool.spawn_keyed(key, state.snapshot(), move |_snapshot| {
            barrier_tx.send(()).unwrap();
        });

        release_tx.send(()).unwrap();
        barrier_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        assert!(!ran.load(Ordering::Acquire));

        let counts = pool.metrics();
        assert_eq!(counts.queued, 3);
        assert_eq!(counts.replaced, 1);
    }

    /// `peak_queue_len` is updated synchronously on `push()`'s caller. Waiting
    /// for the first task's own "started" signal before pushing the next three
    /// guarantees the worker has already dequeued it (see
    /// `test_pool_running_reflects_in_flight_task` for why that signal is safe
    /// to rely on); otherwise a newly spawned worker that hasn't yet dequeued
    /// anything could let all four pile up together.
    #[test]
    fn test_pool_records_peak_queue_len() {
        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        pool.spawn(state.snapshot(), |_snapshot| {});
        pool.spawn(state.snapshot(), |_snapshot| {});
        let (barrier_tx, barrier_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            barrier_tx.send(()).unwrap();
        });

        assert_eq!(pool.metrics().peak_queue_len, 3);

        release_tx.send(()).unwrap();
        barrier_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    /// `running()` reflects a task that has started but not yet returned.
    ///
    /// `started` is bumped before the closure runs, so once this thread
    /// observes the task's own "I've started" signal, the counter update is
    /// guaranteed visible: both happened on the worker thread, in that order,
    /// under the same lock `pool.metrics()` below re-acquires.
    #[test]
    fn test_pool_running_reflects_in_flight_task() {
        let state = WorldState::default();
        let context = Arc::new(LspServiceContext::new());
        let pool = AnalysisPool::with_threads(1, context);

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(pool.metrics().running(), 1);

        release_tx.send(()).unwrap();
    }

    /// Report pool counters if the expected idle notification is lost.
    async fn await_idle(pool: &AnalysisPool) {
        if tokio::time::timeout(Duration::from_secs(10), pool.idle())
            .await
            .is_err()
        {
            panic!(
                "The pool never reported itself idle: {metrics:?}",
                metrics = pool.metrics()
            );
        }
    }

    fn test_pool(threads: usize) -> AnalysisPool {
        AnalysisPool::with_threads(threads, Arc::new(LspServiceContext::new()))
    }

    #[tokio::test]
    async fn test_pool_notifies_idle_on_completed_task() {
        let state = WorldState::default();
        let pool = test_pool(1);

        pool.spawn(state.snapshot(), |_snapshot| {});

        await_idle(&pool).await;
        assert_eq!(pool.metrics().completed, 1);
    }

    #[tokio::test]
    async fn test_pool_notifies_idle_on_task_cancelled_before_starting() {
        let state = WorldState::default();
        let pool = test_pool(1);

        let cancelled = state.snapshot();
        cancelled.cancellation_token().cancel();
        pool.spawn(cancelled, |_snapshot| {});

        await_idle(&pool).await;
        assert_eq!(pool.metrics().cancelled_queued, 1);
    }

    #[tokio::test]
    async fn test_pool_notifies_idle_on_task_cancelled_while_running() {
        let state = WorldState::default();
        let pool = test_pool(1);

        pool.spawn(state.snapshot(), |snapshot| {
            snapshot.cancellation_token().cancel();
            salsa::Database::unwind_if_revision_cancelled(snapshot.db());
        });

        await_idle(&pool).await;
        assert_eq!(pool.metrics().cancelled_running, 1);
    }

    /// Install the production hook so an uncaught worker panic aborts the test
    /// process instead of being ignored.
    #[tokio::test]
    async fn test_pool_notifies_idle_on_panicking_task() {
        crate::panic::install();

        let state = WorldState::default();
        let pool = test_pool(1);

        pool.spawn(state.snapshot(), |_snapshot| {
            panic!("Test panic in an analysis task")
        });

        await_idle(&pool).await;
        assert_eq!(pool.metrics().panicked, 1);
    }

    #[tokio::test]
    async fn test_pool_does_not_notify_idle_while_a_task_runs() {
        let state = WorldState::default();
        let pool = test_pool(2);

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pool.spawn(state.snapshot(), move |_snapshot| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        pool.spawn(state.snapshot(), |_snapshot| {});

        // The completed task cannot retain an idle permit while the gated task
        // is still running.
        assert!(
            tokio::time::timeout(Duration::from_millis(250), pool.idle())
                .await
                .is_err()
        );

        release_tx.send(()).unwrap();
        await_idle(&pool).await;
    }
}
