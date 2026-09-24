//! Diagnostics latency over a real package corpus.
//!
//! Run `just bench` for unretained measurements or `just bench-compare` with
//! Criterion baseline arguments for comparisons. All cases use the optimised
//! `bench` profile without debug assertions. Populate fixtures with
//! `just bench-fixtures` first.
//!
//! The dplyr v1.1.4 corpus contains 109 files and 19k lines under `R/`.
//! It and its eleven CRAN imports live under `target/bench-fixtures/`, so
//! measured runs neither download packages nor use the machine's R library.
//!
//! Most cases are end to end. They send editor notifications to an
//! `LspSession` and time until the main loop accepts the resulting
//! diagnostics, so they cover event handling, scheduling, the analysis pool,
//! and generation filtering. Timing stops at acceptance rather than at
//! settlement, which is only detected on a poll tick. Each iteration settles
//! afterwards, outside the measurement, so leftover work can't overlap the
//! next one.
//!
//! The `ctl.*` cases are controls. They call `generate_diagnostics()` on the
//! bench thread with no main loop or pool. When `one.cold` or `one.warm` moves
//! and its `ctl.*` counterpart doesn't, the change came from scheduling or
//! synchronisation rather than from diagnostics compute.
//!
//! The `ctl.*` and `one.*` cases target `R/mutate.R`, with the remaining dplyr
//! files as workspace context. The `all.*` cases cover every `.R` file under
//! `R/`. `vdoc.burst` replays Quarto virtual-document churn over a small
//! temporary fixture instead of the corpus.
//!
//! Case IDs are limited to 11 characters. Criterion wraps `id` onto its own
//! line when `"diagnostics/".len() + id.len()` exceeds 23, breaking the
//! one-line-per-case output from `just bench`.
//!
//! `OAK_MAX_ANALYSIS_THREADS` pins the analysis pool's worker count for the
//! end-to-end cases. The `ctl.*` cases run on the bench thread and ignore it.
//!
//! Diagnostics resolve base symbols through the `ReadConsole` scopes. The
//! benchmark starts one R session to obtain those scopes, while package
//! resolution remains pinned to the fixture library.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use aether_path::AbsPathBuf;
use aether_path::FilePath;
use anyhow::anyhow;
use ark::lsp::harness::LspHarness;
use ark::lsp::harness::LspSession;
use criterion::measurement::WallTime;
use criterion::BatchSize;
use criterion::BenchmarkGroup;
use criterion::Criterion;
use criterion::SamplingMode;
use oak_core::is_r_file;
use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use oak_scan::ScanScheduler;
use oak_source::SourceCache;
use tokio::runtime::Runtime;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use walkdir::WalkDir;

#[path = "../tests/support/burst_replay.rs"]
mod burst_replay;

/// dplyr v1.1.4. Cloned by tag and checked against this SHA, so the corpus
/// can't drift under a moved tag.
const DPLYR_URL: &str = "https://github.com/tidyverse/dplyr";
const DPLYR_TAG: &str = "v1.1.4";
const DPLYR_SHA: &str = "74de24448833278fc03c8ba5f455aa7c888295b8";

/// dplyr's `Imports`, excluding the base packages `methods` and `utils`.
/// CRAN archives provide the versions released with dplyr v1.1.4.
const PACKAGES: [(&str, &str); 11] = [
    ("R6", "2.5.1"),
    ("cli", "3.6.1"),
    ("generics", "0.1.3"),
    ("glue", "1.6.2"),
    ("lifecycle", "1.0.4"),
    ("magrittr", "2.0.3"),
    ("pillar", "1.9.0"),
    ("rlang", "1.1.2"),
    ("tibble", "3.2.1"),
    ("tidyselect", "1.2.0"),
    ("vctrs", "0.6.5"),
];

/// Representative diagnostics target with nested functions and tidy-eval that
/// clone `DiagnosticContext`.
const TARGET: &str = "R/mutate.R";

/// A file changed by cases that must not edit the diagnostic target.
const UNRELATED: &str = "R/across.R";

type KeyDiagnosticFields<'a> = (&'a str, Option<DiagnosticSeverity>, (u32, u32), (u32, u32));

const WARN: Option<DiagnosticSeverity> = Some(DiagnosticSeverity::WARNING);

/// Expected diagnostics for `R/mutate.R`. The session resolves the base
/// symbols dplyr relies on, so a correct pass over this file reports nothing
/// and any entry here means symbol resolution regressed.
const EXPECTED_TARGET_DIAGNOSTICS: &[KeyDiagnosticFields<'static>] = &[];

/// Counts used by [`bench_all_cold()`] and [`bench_all_warm()`]. Full
/// diagnostic tuples would add about 200 lines after `rustfmt`. These counts
/// detect additions, removals, path changes, and severity changes, but not
/// message or range changes.
type DiagnosticCountByPath<'a> = (&'a str, Option<DiagnosticSeverity>, usize);

/// Expected diagnostics for the pinned corpus, grouped by path and severity.
/// They are known resolution gaps: `vec_order_radix` is not exported by vctrs
/// 0.6.5, and the other missing names belong to `base`, `utils`, dplyr,
/// or packages listed only in dplyr's `Suggests`.
const EXPECTED_ALL_DIAGNOSTIC_COUNTS: &[DiagnosticCountByPath<'static>] = &[
    ("arrange.R", None, 2),
    ("arrange.R", WARN, 1),
    ("colwise.R", None, 2),
    ("compat-dbplyr.R", None, 5),
    ("conditions.R", None, 1),
    ("deprec-dbi.R", None, 10),
    ("deprec-src-local.R", None, 1),
    ("doc-methods.R", None, 2),
    ("nth-value.R", WARN, 1),
    ("order-by.R", WARN, 2),
    ("progress.R", None, 1),
    ("slice.R", None, 5),
    ("src-dbi.R", None, 2),
];

fn main() {
    let fixtures = Fixtures::new();

    if std::env::args().any(|arg| arg == "--populate") {
        if let Err(err) = fixtures.populate() {
            eprintln!("Failed to populate bench fixtures: {err:?}");
            std::process::exit(1);
        }
        return;
    }

    let cache = fixtures.open_source_cache();
    let runtime = session_runtime();

    // Prime Salsa's ingredient-index lookup before collecting measurements.
    let (mut world, target) = setup(&fixtures, &cache);
    assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());
    assert_undefined_symbol_reported(&mut world, &fixtures);
    drop(world);
    assert_session_reports_undefined_symbol(&runtime, &fixtures, &cache);

    let files = match fixtures.r_files() {
        Ok(files) => files,
        Err(err) => panic!("Failed to discover the corpus R files: {err:?}"),
    };
    let relative_paths = fixtures.relative_r_paths(&files);

    // Corpus iterations are expensive. `--sample-size` overrides ten samples.
    let mut criterion = Criterion::default().sample_size(10).configure_from_args();

    // Each group starts from Criterion's configuration. Isolate the longer
    // `all.*` window so it does not extend the single-target cases.

    let mut group = criterion.benchmark_group("diagnostics");
    // Starting sessions and rebuilding corpus databases prevents triangular
    // sampling from fitting the measurement window.
    group.sampling_mode(SamplingMode::Flat);
    bench_control_snapshot(&mut group, &fixtures, &cache);
    bench_control_cold(&mut group, &fixtures, &cache);
    bench_control_warm(&mut group, &fixtures, &cache);
    bench_one_cold(&mut group, &runtime, &fixtures, &cache);
    bench_one_warm(&mut group, &runtime, &fixtures, &cache);
    bench_edit(&mut group, &runtime, &fixtures, &cache);
    bench_open(&mut group, &runtime, &fixtures, &cache);
    bench_symbol(&mut group, &runtime, &fixtures, &cache);
    bench_burst(&mut group, &runtime);
    group.finish();

    let mut group = criterion.benchmark_group("diagnostics");
    group.sampling_mode(SamplingMode::Flat);
    // At about 300 ms per full-corpus iteration, ten samples do not fit the
    // default 3 s.
    group.measurement_time(Duration::from_secs(6));
    bench_all_cold(
        &mut group,
        &runtime,
        &fixtures,
        &cache,
        &files,
        &relative_paths,
    );
    bench_all_warm(
        &mut group,
        &runtime,
        &fixtures,
        &cache,
        &files,
        &relative_paths,
    );
    group.finish();

    criterion.final_summary();
}

/// Control: the per-task clone the main loop pays before a pass even starts.
fn bench_control_snapshot(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    group.bench_function("ctl.snap", |bencher| {
        let (world, target) = setup(fixtures, cache);
        assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());

        bencher.iter(|| world.snapshot());
    });
}

/// Control: the first pass on a prepared database, on the bench thread: parse,
/// index, and the pass itself.
fn bench_control_cold(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    group.bench_function("ctl.cold", |bencher| {
        bencher.iter_batched_ref(
            || setup(fixtures, cache),
            |(world, target)| {
                let diagnostics = world.diagnose(target, world.snapshot()).unwrap();
                assert_diagnostics(&diagnostics);
            },
            // Each prepared corpus database is too large to batch.
            BatchSize::PerIteration,
        );
    });
}

/// Control: the target again in the same revision, against warm memos, on the
/// bench thread.
fn bench_control_warm(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    group.bench_function("ctl.warm", |bencher| {
        let (world, target) = setup(fixtures, cache);
        assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());

        bencher.iter(|| {
            let diagnostics = world.diagnose(&target, world.snapshot()).unwrap();
            assert_diagnostics(&diagnostics);
        });
    });
}

/// The target's first diagnostics after startup: the `didOpen` handler, the
/// scheduler, and a pass on a pool thread. Startup has already scanned and
/// warmed the workspace index, as it has by the time a user opens a file.
fn bench_one_cold(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let target = fixtures.dplyr().join(TARGET);
    let contents = read_source(&target);

    group.bench_function("one.cold", |bencher| {
        bencher.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let mut session = start_session(runtime, fixtures, cache);
                let (diagnostics, elapsed) = measure(runtime, &mut session, async |session| {
                    session.send_did_open(&target, &contents);
                    session.wait_for_accepted_diagnostics(&target, 0).await
                });
                assert_diagnostics(&diagnostics);
                total += elapsed;
                end_session(runtime, session);
            }
            total
        });
    });
}

/// Change the target without altering its text. The revision advances, so the
/// target re-parses and its pass reruns against otherwise warm memos. This is
/// the single-file counterpart of `all.warm`.
fn bench_one_warm(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let target = fixtures.dplyr().join(TARGET);
    let contents = read_source(&target);

    group.bench_function("one.warm", |bencher| {
        bencher.iter_custom(|iters| {
            let mut session = start_session_with_target(runtime, fixtures, cache);
            let mut total = Duration::ZERO;
            for version in document_versions(iters) {
                let (diagnostics, elapsed) = measure(runtime, &mut session, async |session| {
                    session.send_did_change(&target, &contents, version);
                    session
                        .wait_for_accepted_diagnostics(&target, version)
                        .await
                });
                assert_diagnostics(&diagnostics);
                total += elapsed;
            }
            end_session(runtime, session);
            total
        });
    });
}

/// Edit the target and wait for its refresh. The edit alternates between
/// appending a comment and restoring the original, so every iteration changes
/// the text while one session serves every iteration of a sample.
fn bench_edit(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let target = fixtures.dplyr().join(TARGET);
    let original = read_source(&target);
    let edited = format!("{original}\n# A comment\n");

    group.bench_function("one.edit", |bencher| {
        bencher.iter_custom(|iters| {
            let mut session = start_session_with_target(runtime, fixtures, cache);
            let mut total = Duration::ZERO;
            for version in document_versions(iters) {
                let contents = if version % 2 == 1 { &edited } else { &original };
                let (diagnostics, elapsed) = measure(runtime, &mut session, async |session| {
                    session.send_did_change(&target, contents, version);
                    session
                        .wait_for_accepted_diagnostics(&target, version)
                        .await
                });
                assert_diagnostics(&diagnostics);
                total += elapsed;
            }
            end_session(runtime, session);
            total
        });
    });
}

/// An unrelated file opens with its on-disk contents. No symbol changes, but
/// the revision advances, so the target is refreshed along with the new
/// buffer. The close that resets the next iteration is not measured.
fn bench_open(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let target = fixtures.dplyr().join(TARGET);
    let unrelated = fixtures.dplyr().join(UNRELATED);
    let contents = read_source(&unrelated);

    group.bench_function("one.open", |bencher| {
        bencher.iter_custom(|iters| {
            let mut session = start_session_with_target(runtime, fixtures, cache);
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let (accepted, elapsed) = measure(runtime, &mut session, async |session| {
                    session.send_did_open(&unrelated, &contents);
                    session
                        .wait_for_all_accepted_diagnostics(&[(&target, 0), (&unrelated, 0)])
                        .await
                });
                assert_diagnostics(&accepted[0]);
                total += elapsed;
                close_document(runtime, &mut session, &unrelated);
            }
            end_session(runtime, session);
            total
        });
    });
}

/// Open a new file that defines a top-level symbol, invalidating the
/// workspace symbols the target pass reads. Each iteration opens a fresh
/// path, so none resurrects a buffer closed by an earlier iteration.
fn bench_symbol(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let target = fixtures.dplyr().join(TARGET);

    group.bench_function("one.symbol", |bencher| {
        bencher.iter_custom(|iters| {
            let mut session = start_session_with_target(runtime, fixtures, cache);
            let mut total = Duration::ZERO;
            for iteration in 0..iters {
                let path = fixtures
                    .dplyr()
                    .join(format!("R/zzz-bench-new-symbol-{iteration}.R"));
                let (accepted, elapsed) = measure(runtime, &mut session, async |session| {
                    session.send_did_open(&path, "bench_new_symbol <- function() NULL\n");
                    session
                        .wait_for_all_accepted_diagnostics(&[(&target, 0), (&path, 0)])
                        .await
                });
                assert_diagnostics(&accepted[0]);
                total += elapsed;
                close_document(runtime, &mut session, &path);
            }
            end_session(runtime, session);
            total
        });
    });
}

/// Measure final recurring-document diagnostics and probe completion, not full
/// settlement. Concurrent workspace scans and source ingestion may outlast
/// this endpoint. The `burst` integration test reports settlement time as well
/// so work outside the measured interval remains visible.
fn bench_burst(group: &mut BenchmarkGroup<'_, WallTime>, runtime: &Runtime) {
    group.bench_function("vdoc.burst", |bencher| {
        bencher.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let fixture = burst_replay::Fixture::new();
                let mut replay = runtime.block_on(burst_replay::Replay::start(
                    &fixture,
                    burst_replay::BurstConfig::DEFAULT,
                ));
                replay.enqueue_burst(&fixture);

                let start = Instant::now();
                runtime.block_on(replay.drive_to_endpoint(&fixture));
                total += start.elapsed();

                runtime.block_on(replay.settle());
                replay.assert_final_state(&fixture);
                end_session(runtime, replay.session);
            }
            total
        });
    });
}

/// Open every corpus file at once, as when an editor restores a session, and
/// wait until each has published. Every `didOpen` advances the revision and
/// refreshes all files opened so far, so this includes keyed replacement in
/// the pool under a burst.
fn bench_all_cold(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
    relative_paths: &[String],
) {
    let contents: Vec<String> = files.iter().map(|file| read_source(file)).collect();
    let targets: Vec<(&Path, i32)> = files.iter().map(|file| (file.as_path(), 0)).collect();

    group.bench_function("all.cold", |bencher| {
        bencher.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let mut session = start_session(runtime, fixtures, cache);
                let (accepted, elapsed) = measure(runtime, &mut session, async |session| {
                    for (file, contents) in files.iter().zip(&contents) {
                        session.send_did_open(file, contents);
                    }
                    session.wait_for_all_accepted_diagnostics(&targets).await
                });
                assert_all_accepted(relative_paths, &accepted);
                total += elapsed;
                end_session(runtime, session);
            }
            total
        });
    });
}

/// With every corpus file open and diagnosed, change `UNRELATED` without
/// altering its text. The revision advances and every open file refreshes:
/// that one re-parses and the rest revalidate warm memos.
fn bench_all_warm(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
    relative_paths: &[String],
) {
    let contents: Vec<String> = files.iter().map(|file| read_source(file)).collect();
    let unrelated = fixtures.dplyr().join(UNRELATED);
    let unrelated_contents = read_source(&unrelated);

    group.bench_function("all.warm", |bencher| {
        bencher.iter_custom(|iters| {
            let mut session = start_session(runtime, fixtures, cache);
            runtime.block_on(async {
                for (file, contents) in files.iter().zip(&contents) {
                    session.send_did_open(file, contents);
                }
                session.settle().await;
            });

            let mut total = Duration::ZERO;
            for version in document_versions(iters) {
                let targets: Vec<(&Path, i32)> = files
                    .iter()
                    .map(|file| (file.as_path(), if *file == unrelated { version } else { 0 }))
                    .collect();
                let (accepted, elapsed) = measure(runtime, &mut session, async |session| {
                    session.send_did_change(&unrelated, &unrelated_contents, version);
                    session.wait_for_all_accepted_diagnostics(&targets).await
                });
                assert_all_accepted(relative_paths, &accepted);
                total += elapsed;
            }
            end_session(runtime, session);
            total
        });
    });
}

/// Time `work`, then settle outside the measurement so leftover refreshes
/// can't overlap the next iteration.
fn measure<T>(
    runtime: &Runtime,
    session: &mut LspSession,
    work: impl AsyncFnOnce(&mut LspSession) -> T,
) -> (T, Duration) {
    let start = Instant::now();
    let output = runtime.block_on(work(session));
    let elapsed = start.elapsed();

    runtime.block_on(session.settle());
    (output, elapsed)
}

/// Versions `1..=iters` for successive changes to a document opened at 0.
fn document_versions(iters: u64) -> impl Iterator<Item = i32> {
    let Ok(last) = i32::try_from(iters) else {
        panic!("Too many iterations for LSP document versions: {iters}");
    };
    1..=last
}

fn close_document(runtime: &Runtime, session: &mut LspSession, path: &Path) {
    session.send_did_close(path);
    runtime.block_on(session.settle());
}

/// A database over the fixture corpus with the target open, before any pass.
fn setup(fixtures: &Fixtures, cache: &SourceCache) -> (LspHarness, FilePath) {
    match build_world(fixtures, cache) {
        Ok(world) => world,
        Err(err) => panic!("Failed to build the bench world: {err:?}"),
    }
}

/// The session's main loop and simulated editor run on the bench thread, as
/// in `#[tokio::test]`. Analysis tasks run on the pool's own threads.
fn session_runtime() -> Runtime {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => panic!("Failed to build the session runtime: {err:?}"),
    }
}

/// A settled session over the fixture corpus with no document open.
fn start_session(runtime: &Runtime, fixtures: &Fixtures, cache: &SourceCache) -> LspSession {
    let harness = match build_world_base(fixtures, cache) {
        Ok(harness) => harness,
        Err(err) => panic!("Failed to build the bench world: {err:?}"),
    };
    runtime.block_on(harness.start(&[]))
}

/// A settled session with the target open and diagnosed once.
fn start_session_with_target(
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) -> LspSession {
    let mut session = start_session(runtime, fixtures, cache);
    let target = fixtures.dplyr().join(TARGET);

    runtime.block_on(async {
        session.send_did_open(&target, &read_source(&target));
        assert_diagnostics(&session.wait_for_accepted_diagnostics(&target, 0).await);
        session.settle().await;
    });

    session
}

/// Drop inside the runtime because the simulated editor's peer task belongs
/// to it.
fn end_session(runtime: &Runtime, session: LspSession) {
    runtime.block_on(async move { drop(session) });
}

fn build_world(fixtures: &Fixtures, cache: &SourceCache) -> anyhow::Result<(LspHarness, FilePath)> {
    let mut world = build_world_base(fixtures, cache)?;

    let target = fixtures.workspace_path(TARGET);
    world.prepare_document(&target, read_source(&fixtures.dplyr().join(TARGET)))?;

    Ok((world, target))
}

/// Initialize shared scan, package, and console-scope state without opening
/// editor buffers, which each case controls.
fn build_world_base(fixtures: &Fixtures, cache: &SourceCache) -> anyhow::Result<LspHarness> {
    if !fixtures.dplyr().is_dir() {
        return Err(missing_fixture("the dplyr checkout"));
    }

    let mut db = OakDatabase::new();

    // The fixture library registers each import as installed. Its sources come
    // from cached CRAN tarballs rather than the machine's R library.
    db.set_library_paths(&[fixtures.library()]);
    for (name, version) in PACKAGES {
        let root = cache
            .get_cran(name, version)
            .ok_or_else(|| missing_fixture(&format!("CRAN sources for {name} {version}")))?;
        let package = db
            .package_by_name(name)
            .ok_or_else(|| missing_fixture(&format!("library entry for {name}")))?;
        db.set_package_sources(package, &root.join("R"));
    }

    let mut scheduler = ScanScheduler::new();
    let editor_owned = HashSet::new();
    let requests = scheduler.set_workspace_paths(&mut db, &[fixtures.dplyr()], &editor_owned);
    for request in requests {
        let completed = request.run();
        scheduler.apply_scan_completed(&mut db, completed, &editor_owned);
    }

    let mut world = LspHarness::new(db);
    world.set_workspace_folders(vec![fixtures.workspace_folder()]);
    world.set_installed_packages(PACKAGES.iter().map(|(name, _)| name.to_string()).collect());
    world.set_console_scopes(session_scopes().clone());

    Ok(world)
}

/// Started once and shared by every case, because a session boot costs more
/// than the passes being measured.
fn session_scopes() -> &'static Vec<Vec<String>> {
    static SCOPES: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    SCOPES.get_or_init(ark::lsp::harness::r_session_scopes)
}

#[track_caller]
fn assert_diagnostics(diagnostics: &[Diagnostic]) {
    assert_eq!(key_fields(diagnostics), EXPECTED_TARGET_DIAGNOSTICS);
}

#[track_caller]
fn assert_all_accepted(relative_paths: &[String], accepted: &[Vec<Diagnostic>]) {
    let per_file: Vec<(&str, Vec<Diagnostic>)> = relative_paths
        .iter()
        .map(String::as_str)
        .zip(accepted.iter().cloned())
        .collect();
    assert_eq!(
        diagnostic_counts_by_path(&per_file),
        EXPECTED_ALL_DIAGNOSTIC_COUNTS
    );
}

/// The session counterpart of [`assert_undefined_symbol_reported()`]. It
/// checks that the settings the session pulls leave diagnostics enabled.
#[track_caller]
fn assert_session_reports_undefined_symbol(
    runtime: &Runtime,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let mut session = start_session(runtime, fixtures, cache);
    let path = fixtures.dplyr().join("R/zzz-bench-undefined.R");

    let diagnostics = runtime.block_on(async {
        session.send_did_open(&path, "zzz_bench_undefined\n");
        session.wait_for_accepted_diagnostics(&path, 0).await
    });
    assert_eq!(key_fields(&diagnostics), [(
        "No symbol named 'zzz_bench_undefined' in scope.",
        WARN,
        (0, 0),
        (0, 19)
    )]);

    end_session(runtime, session);
}

/// Verify that diagnostics are enabled, since a clean target also produces no
/// diagnostics when the pass is unconfigured or disabled.
#[track_caller]
fn assert_undefined_symbol_reported(world: &mut LspHarness, fixtures: &Fixtures) {
    let path = fixtures.workspace_path("R/zzz-bench-undefined.R");
    world
        .prepare_document(&path, String::from("zzz_bench_undefined\n"))
        .unwrap();

    let diagnostics = world.diagnose(&path, world.snapshot()).unwrap();
    assert_eq!(key_fields(&diagnostics), [(
        "No symbol named 'zzz_bench_undefined' in scope.",
        WARN,
        (0, 0),
        (0, 19)
    )]);

    world.close_document(&path);
}

fn key_fields(diagnostics: &[Diagnostic]) -> Vec<KeyDiagnosticFields<'_>> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.message.as_str(),
                diagnostic.severity,
                (
                    diagnostic.range.start.line,
                    diagnostic.range.start.character,
                ),
                (diagnostic.range.end.line, diagnostic.range.end.character),
            )
        })
        .collect()
}

/// `BTreeMap` orders `None` before `Some`, matching the severity order in
/// [`EXPECTED_ALL_DIAGNOSTIC_COUNTS`] within each path.
fn diagnostic_counts_by_path<'a>(
    per_file: &[(&'a str, Vec<Diagnostic>)],
) -> Vec<DiagnosticCountByPath<'a>> {
    per_file
        .iter()
        .flat_map(|(path, diagnostics)| {
            let mut by_severity = BTreeMap::new();
            for diagnostic in diagnostics {
                *by_severity.entry(diagnostic.severity).or_insert(0usize) += 1;
            }
            by_severity
                .into_iter()
                .map(move |(severity, count)| (*path, severity, count))
        })
        .collect()
}

/// All benchmark fixtures under `target/bench-fixtures/`.
struct Fixtures {
    root: PathBuf,
}

impl Fixtures {
    fn new() -> Self {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap_or_else(|| panic!("Can't find the workspace root above the ark crate"));

        Self {
            root: workspace.join("target").join("bench-fixtures"),
        }
    }

    fn dplyr(&self) -> PathBuf {
        self.root.join("dplyr")
    }

    /// Provides `DESCRIPTION` and `NAMESPACE` stubs so package discovery treats
    /// cached imports as installed.
    fn library(&self) -> PathBuf {
        self.root.join("library")
    }

    fn source_cache(&self) -> PathBuf {
        self.root.join("source")
    }

    fn open_source_cache(&self) -> SourceCache {
        match SourceCache::open_in(self.source_cache()) {
            Ok(cache) => cache,
            Err(err) => panic!("Failed to open the fixture source cache: {err:?}"),
        }
    }

    fn workspace_folder(&self) -> AbsPathBuf {
        match FilePath::from_path_buf(self.dplyr()).and_then(|path| path.as_file().cloned()) {
            Some(path) => path,
            None => panic!("The dplyr fixture is not an absolute path"),
        }
    }

    fn workspace_path(&self, relative: &str) -> FilePath {
        self.file_path(self.dplyr().join(relative))
    }

    fn file_path(&self, absolute: PathBuf) -> FilePath {
        match FilePath::from_path_buf(absolute) {
            Some(path) => path,
            None => panic!("The dplyr fixture is not an absolute path"),
        }
    }

    /// Include nested source files and sort them for deterministic case order.
    fn r_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let r_dir = self.dplyr().join("R");

        let mut files: Vec<PathBuf> = WalkDir::new(&r_dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| is_r_file(path))
            .collect();

        if files.is_empty() {
            return Err(anyhow!("No R files found under {}", r_dir.display()));
        }

        files.sort();
        Ok(files)
    }

    /// Strips the fixture's `R/` prefix so diagnostic keys do not depend on the
    /// corpus's absolute path.
    fn relative_r_paths(&self, files: &[PathBuf]) -> Vec<String> {
        let r_dir = self.dplyr().join("R");
        files
            .iter()
            .map(|file| {
                file.strip_prefix(&r_dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// Download fixtures before benchmarking so measured runs need no network.
    fn populate(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        self.clone_dplyr()?;

        let cache = SourceCache::open_in(self.source_cache())?;
        for (name, version) in PACKAGES {
            let root = match cache.get_cran(name, version) {
                Some(root) => root,
                None => cache
                    .insert_cran(name, version)
                    .ok_or_else(|| anyhow!("Can't download {name} {version} from CRAN"))?,
            };

            let package = self.library().join(name);
            std::fs::create_dir_all(&package)?;
            for metadata in ["DESCRIPTION", "NAMESPACE"] {
                std::fs::write(package.join(metadata), std::fs::read(root.join(metadata))?)?;
            }
        }

        Ok(())
    }

    /// Verify `DPLYR_SHA` because `DPLYR_TAG` can move and existing checkouts
    /// can drift.
    fn clone_dplyr(&self) -> anyhow::Result<()> {
        if !self.dplyr().is_dir() {
            run_git(&[
                "clone",
                "--filter=blob:none",
                "--single-branch",
                "--branch",
                DPLYR_TAG,
                DPLYR_URL,
                &self.dplyr().to_string_lossy(),
            ])?;
        }

        let head = run_git(&["-C", &self.dplyr().to_string_lossy(), "rev-parse", "HEAD"])?;
        if head.trim() != DPLYR_SHA {
            return Err(anyhow!(
                "{DPLYR_TAG} resolved to {head}, expected {DPLYR_SHA}. Delete {dir} and retry.",
                head = head.trim(),
                dir = self.dplyr().display()
            ));
        }

        Ok(())
    }
}

fn run_git(args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new("git").args(args).output()?;

    if !output.status.success() {
        return Err(anyhow!(
            "`git {args}` failed: {stderr}",
            args = args.join(" "),
            stderr = String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8(output.stdout)?)
}

fn read_source(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => panic!("Can't read {path}: {err}", path = path.display()),
    }
}

fn missing_fixture(what: &str) -> anyhow::Error {
    anyhow!("Missing {what}. Populate the fixtures with `just bench-fixtures`.")
}
