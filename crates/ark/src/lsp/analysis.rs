//! Background analysis, the only place a salsa db handle lives off the main
//! loop.
//!
//! Two things maintain that invariant: [`WorldStateSnapshot`] is built only in
//! this module, and `OakDatabase` isn't `Clone`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;

use aether_path::FilePath;
use oak_db::OakDatabase;
use stdext::result::ResultExt;
use stdext::spawn;

use crate::lsp;
use crate::lsp::config::LspConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::indexer;
use crate::lsp::io_pool::panic_message;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::open_file::OpenFile;
use crate::lsp::state::Workspace;
use crate::lsp::state::WorldState;
use crate::url::FilePathExt;

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
    fn spawn(
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
    fn spawn_keyed(
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

/// A diagnostics task's result on its way back to the main loop. The generation
/// state enables [`DiagnosticsState::accept`] to distinguish a stale result
/// from a fresh one.
#[derive(Debug)]
pub(crate) struct DiagnosticsReady {
    pub(crate) generation: u64,
    pub(crate) publication: DiagnosticsPublication,
}

/// Tracks diagnostics staleness across refresh batches, so an out-of-order
/// result gets dropped instead of published over a newer one.
///
/// Mirrors rust-analyzer's generation counter in
/// `crates/rust-analyzer/src/diagnostics.rs`.
#[derive(Default)]
pub(crate) struct DiagnosticsState {
    /// Bumped once per refresh batch.
    generation: u64,
    /// Generation of the newest result published per file.
    published: HashMap<FilePath, u64>,
}

impl DiagnosticsState {
    /// Queue a diagnostics pass for every open file we diagnose, all tagged
    /// with a new generation.
    pub(crate) fn refresh_all(
        &mut self,
        state: &WorldState,
        pool: &AnalysisPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        self.generation += 1;
        let generation = self.generation;

        let files: Vec<(&FilePath, &OpenFile)> = state
            .open_files
            .iter()
            .filter(|(path, _open_file)| path.should_diagnose())
            .collect();

        tracing::trace!("Refreshing diagnostics for {n} documents", n = files.len());
        lsp::log_info!("Queueing {n} diagnostic tasks", n = files.len());

        for (path, open_file) in files {
            let path = path.clone();
            let file = open_file.clone();
            let events_tx = events_tx.clone();

            pool.spawn_keyed(path.clone(), state.snapshot(), move |snapshot| {
                let publication = refresh_diagnostics(path, file, snapshot);
                let ready = DiagnosticsReady {
                    generation,
                    publication,
                };
                events_tx.send(Event::DiagnosticsReady(ready)).log_err();
            });
        }
    }

    /// Whether a diagnostics result for `path` computed at `generation`
    /// should be published now, or is stale and should be dropped.
    ///
    /// Equal generations can't legitimately arrive twice for the same file:
    /// we spawn one task per file per batch, and keyed replacement on the
    /// pool keeps at most one queued entry per file.
    pub(crate) fn accept(&mut self, path: &FilePath, generation: u64) -> bool {
        if let Some(published) = self.published.get(path) {
            if *published > generation {
                return false;
            }
        }

        self.published.insert(path.clone(), generation);
        true
    }

    /// Generation of the newest result already published for `path`, for the
    /// main loop to log alongside a dropped stale result.
    pub(crate) fn published_generation(&self, path: &FilePath) -> Option<u64> {
        self.published.get(path).copied()
    }
}

fn refresh_diagnostics(
    path: FilePath,
    file: OpenFile,
    state: WorldStateSnapshot,
) -> DiagnosticsPublication {
    let uri = file.wire_uri().clone();
    let version = file.version();
    let _span = tracing::info_span!("diagnostics_refresh", uri = %uri.as_str()).entered();

    // Special case testthat-specific behaviour. This is a simple stopgap
    // approach that has some false positives (e.g. when we work on testthat
    // itself the flag will always be true), but that shouldn't have much
    // practical impact.
    let testthat = path
        .as_path()
        .is_some_and(|path| path.components().any(|c| c.as_str() == "testthat"));

    let now = std::time::Instant::now();
    lsp::log_info!("Generating diagnostics for file: {}", uri.as_str());

    let diagnostics = generate_diagnostics(file.file(), state, testthat);

    lsp::log_info!(
        "Finished diagnostics for file: {} in {:.0?}",
        uri.as_str(),
        now.elapsed()
    );

    DiagnosticsPublication {
        path,
        uri,
        diagnostics,
        version,
    }
}

/// Build the per-file workspace symbol indexes on a background thread so
/// main-loop consumers triggered by the user (workspace symbols, workspace
/// completions) find them already computed. The first run after a workspace
/// scan does the real work, parsing and walking each file. Later runs only
/// revalidate the per-file memos.
///
/// Mirrors rust-analyzer's cache warming: spawned when a workspace scan
/// settles, the analogue of r-a's transitions to quiescence (initial VFS scan,
/// workspace reload, etc). Unlike r-a we don't restart a warmup that gets
/// cancelled (the pool swallows the unwind). A cancelling write can only come
/// from an editor buffer, so a document is open, and the diagnostics passes
/// spawned by that same write force the same memos and finish the job.
pub(crate) fn warm_workspace_index(state: &WorldState, pool: &AnalysisPool) {
    pool.spawn(state.snapshot(), |snapshot| {
        let now = std::time::Instant::now();
        lsp::log_info!("Starting workspace index warmup");
        indexer::warm(snapshot.db());
        lsp::log_info!("Finished workspace index warmup ({:.0?})", now.elapsed());
    })
}

/// Read-only snapshot of [`WorldState`] handed to a background reader, so a
/// reader thread can't reach salsa input setters. Carries only the fields
/// readers actually use. Mirrors rust-analyzer's `GlobalStateSnapshot`.
#[derive(Debug)]
pub(crate) struct WorldStateSnapshot {
    /// Private so readers can only reach it through [`Self::db`].
    db: OakDatabase,
    pub(crate) workspace: Workspace,
    pub(crate) console_scopes: Vec<Vec<String>>,
    pub(crate) installed_packages: Vec<String>,
    pub(crate) config: LspConfig,
}

/// Minting lives here rather than in `state.rs` because
/// [`WorldStateSnapshot`]'s db field is private to this module.
impl WorldState {
    /// Take a read-only snapshot of the world for a background reader.
    ///
    /// The snapshot holds a Salsa handle, which parks the next main-loop write
    /// until it drops. That's safe on the [`AnalysisPool`], whose tasks unwind
    /// on cancellation. The only other caller is `handle_completion()`, which
    /// hands the snapshot to `r_task()` and blocks until it returns, so that
    /// handle can't outlive the tick that made it.
    pub(crate) fn snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            db: self.db.snapshot(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            config: self.config.clone(),
            workspace: self.workspace.clone(),
        }
    }
}

impl WorldStateSnapshot {
    /// Read-only access to the database. Returns `&dyn ArkDb` rather than
    /// `&OakDatabase` because `dyn ArkDb` is unsized, so a reader can't
    /// `.snapshot()` its way to an owned database and call setters on it.
    pub(crate) fn db(&self) -> &dyn ArkDb {
        &self.db
    }

    /// Whether salsa would unwind this handle's next query, because a writer is
    /// parked waiting for it to drop (or, in tests, because the token was armed
    /// by hand). `unwind_if_revision_cancelled()` reports by throwing, so we
    /// catch it to get an answer.
    fn is_cancelled(&self) -> bool {
        catch_cancellation(|| salsa::Database::unwind_if_revision_cancelled(&self.db)).is_none()
    }

    /// The database's salsa cancellation token. Read-side only: it observes and
    /// arms cancellation, it doesn't mutate any input. Only cancellation tests
    /// arm it by hand.
    #[cfg(test)]
    pub(crate) fn cancellation_token(&self) -> salsa::CancellationToken {
        salsa::Database::cancellation_token(&self.db)
    }
}

/// Run `f`, swallowing a salsa cancellation as `None`. Any other panic propagates.
fn catch_cancellation<T>(f: impl FnOnce() -> T) -> Option<T> {
    salsa::Cancelled::catch(AssertUnwindSafe(f)).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use aether_path::FilePath;
    use oak_scan::DbScan;
    use url::Url;

    use super::catch_cancellation;
    use super::refresh_diagnostics;
    use super::AnalysisPool;
    use super::DiagnosticsState;
    use crate::lsp::state::WorldState;
    use crate::lsp::traits::url::UrlExt;

    /// `accept` is the staleness gate a refresh batch relies on: a result only
    /// publishes if no newer generation for that file already went out. Pins
    /// the three cases that can arrive at the main loop: a file seen for the
    /// first time, a fresh batch superseding the last one, and a straggler
    /// from an old batch arriving after a newer one already landed. Also pins
    /// the deliberate choice to accept a repeat of the last generation (see
    /// `accept`'s doc comment for why that can't happen in practice but is
    /// still safe).
    #[test]
    fn test_accept_tracks_staleness_per_file() {
        let mut diagnostics = DiagnosticsState::default();
        let path = FilePath::from_url(&Url::parse("file:///test.R").unwrap());

        assert!(diagnostics.accept(&path, 1));
        assert!(diagnostics.accept(&path, 2));
        assert!(!diagnostics.accept(&path, 1));
        assert!(diagnostics.accept(&path, 2));
    }

    /// A salsa cancellation during the pass is swallowed into `None` by
    /// `catch_cancellation`, the wrapper the pool applies to every task, rather
    /// than unwinding and killing the worker thread.
    ///
    /// `cancellation_token().cancel()` arms local cancellation on the snapshot's
    /// oak, so the first salsa query in `generate_diagnostics` (the `tree_sitter`
    /// fetch) unwinds with `salsa::Cancelled`, the same payload a concurrent
    /// `set_*` produces. The unwind fires before any R, so no `r_task` here.
    #[test]
    fn test_cancelled_diagnostics_pass_is_caught() {
        let mut state = WorldState::default();
        let uri = Url::parse("file:///test.R").unwrap();
        let path = FilePath::from_url(&uri);
        let code = "foo";
        let file = state.db.upsert_editor(path.clone(), code.to_string());
        state.insert_open_file(uri.to_uri().unwrap(), path.clone(), file, None);

        let file = state.open_file(&path).unwrap().clone();
        let snapshot = state.snapshot();
        snapshot.cancellation_token().cancel();

        assert!(catch_cancellation(|| refresh_diagnostics(path, file, snapshot)).is_none());
    }

    /// A snapshot reports itself cancelled once salsa would unwind its next
    /// query, which is what the pool checks at dequeue.
    #[test]
    fn test_cancelled_snapshot_reports_cancelled() {
        let state = WorldState::default();

        let live = state.snapshot();
        assert!(!live.is_cancelled());

        let cancelled = state.snapshot();
        cancelled.cancellation_token().cancel();
        assert!(cancelled.is_cancelled());
    }

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
