//! Baseline for one diagnostics pass over a real package corpus.
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
//! The `one.*` cases diagnose `R/mutate.R` with the remaining dplyr files as
//! workspace context. The `all.*` cases diagnose every `.R` file under `R/`
//! with a fresh snapshot per file. `.cold` measures the first pass, while
//! `.warm` repeats it against warm memos.
//!
//! Case IDs are limited to 11 characters. Criterion wraps `id` onto its own
//! line when `"diagnostics/".len() + id.len()` exceeds 23, breaking the
//! one-line-per-case output from `just bench`.
//!
//! Every case calls `generate_diagnostics()` on the bench thread. No analysis
//! thread is involved, so `ARK_MAX_ANALYSIS_THREADS` has no effect.
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

use aether_path::AbsPathBuf;
use aether_path::FilePath;
use anyhow::anyhow;
use ark::lsp::harness::LspHarness;
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
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use walkdir::WalkDir;

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

    // Prime Salsa's ingredient-index lookup before collecting measurements.
    let (mut world, target) = setup(&fixtures, &cache);
    assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());
    assert_undefined_symbol_reported(&mut world, &fixtures);
    drop(world);

    let files = match fixtures.r_files() {
        Ok(files) => files,
        Err(err) => panic!("Failed to discover the corpus R files: {err:?}"),
    };
    let relative_paths = fixtures.relative_r_paths(&files);

    // Corpus iterations are expensive. `--sample-size` overrides ten samples.
    let mut criterion = Criterion::default().sample_size(10).configure_from_args();

    // Each group starts from Criterion's configuration. Isolate the longer
    // `all.*` window so it does not extend mutation cases.

    let mut group = criterion.benchmark_group("diagnostics");
    // Rebuilding corpus databases prevents triangular sampling from fitting the
    // measurement window.
    group.sampling_mode(SamplingMode::Flat);
    bench_snapshot(&mut group, &fixtures, &cache);
    bench_cold(&mut group, &fixtures, &cache);
    bench_warm_repeat(&mut group, &fixtures, &cache);
    group.finish();

    let mut group = criterion.benchmark_group("diagnostics");
    group.sampling_mode(SamplingMode::Flat);
    // At about 300 ms per full-corpus iteration, ten samples do not fit the
    // default 3 s.
    group.measurement_time(Duration::from_secs(6));
    bench_all_cold(&mut group, &fixtures, &cache, &files, &relative_paths);
    bench_all_warm(&mut group, &fixtures, &cache, &files, &relative_paths);
    group.finish();

    let mut group = criterion.benchmark_group("diagnostics");
    group.sampling_mode(SamplingMode::Flat);
    bench_unrelated_open_close(&mut group, &fixtures, &cache);
    bench_unrelated_new_symbol(&mut group, &fixtures, &cache);
    bench_target_edit(&mut group, &fixtures, &cache);
    group.finish();

    criterion.final_summary();
}

/// The per-task clone the main loop pays before a pass even starts.
fn bench_snapshot(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    group.bench_function("snapshot", |bencher| {
        let (world, target) = setup(fixtures, cache);
        assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());

        bencher.iter(|| world.snapshot());
    });
}

/// First pass on a prepared database: parse, index, and the pass itself.
fn bench_cold(group: &mut BenchmarkGroup<'_, WallTime>, fixtures: &Fixtures, cache: &SourceCache) {
    group.bench_function("one.cold", |bencher| {
        bencher.iter_batched_ref(
            || setup(fixtures, cache),
            |(world, target)| {
                let diagnostics = world.diagnose(target, world.snapshot()).unwrap();
                assert_diagnostics(&diagnostics);
            },
            // Use `PerIteration`. Each prepared corpus database is too large to batch.
            BatchSize::PerIteration,
        );
    });
}

/// The same file again in the same revision, against warm memos.
fn bench_warm_repeat(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    group.bench_function("one.warm", |bencher| {
        let (world, target) = setup(fixtures, cache);
        assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());

        bencher.iter(|| {
            let diagnostics = world.diagnose(&target, world.snapshot()).unwrap();
            assert_diagnostics(&diagnostics);
        });
    });
}

/// An unrelated file opens with the contents it already has on disk and closes
/// again, so the revision advances twice without any symbol or source
/// relationship changing.
fn bench_unrelated_open_close(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let mutate = |world: &mut LspHarness, fixtures: &Fixtures| {
        let path = fixtures.workspace_path(UNRELATED);
        let contents = read_source(&fixtures.dplyr().join(UNRELATED));
        world.prepare_document(&path, contents).unwrap();
        world.close_document(&path);
    };

    bench_mutation_then_pass(group, fixtures, cache, "open", "update", mutate);
}

/// Open a new file that defines a top-level symbol, invalidating the
/// workspace-symbol dependency read by the target pass.
fn bench_unrelated_new_symbol(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let mutate = |world: &mut LspHarness, fixtures: &Fixtures| {
        let path = fixtures.workspace_path("R/zzz-bench-new-symbol.R");
        world
            .prepare_document(&path, String::from("bench_new_symbol <- function() NULL\n"))
            .unwrap();
    };

    bench_mutation_then_pass(group, fixtures, cache, "symbol", "add", mutate);
}

/// Edit the diagnosed file itself, unlike the unrelated-file cases.
fn bench_target_edit(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
) {
    let mutate = |world: &mut LspHarness, fixtures: &Fixtures| {
        let path = fixtures.workspace_path(TARGET);
        let edited = format!("{}\n# A comment\n", world.source_text(&path).unwrap());
        world.prepare_document(&path, edited).unwrap();
    };

    bench_mutation_then_pass(group, fixtures, cache, "edit", "update", mutate);
}

/// Uses a fresh snapshot for each file to include the per-task snapshot cost
/// that a workspace-wide snapshot would hide.
fn bench_all_cold(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
    relative_paths: &[String],
) {
    group.bench_function("all.cold", |bencher| {
        bencher.iter_batched_ref(
            || setup_all(fixtures, cache, files),
            |(world, targets)| diagnose_all(world, targets, relative_paths),
            // A prepared corpus database is too large to batch.
            BatchSize::PerIteration,
        );
    });
}

/// Repeat the full-corpus pass against warm memos.
fn bench_all_warm(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
    relative_paths: &[String],
) {
    group.bench_function("all.warm", |bencher| {
        let (world, targets) = setup_all_warm(fixtures, cache, files, relative_paths);

        bencher.iter(|| diagnose_all(&world, &targets, relative_paths));
    });
}

/// Keep per-file snapshot, diagnostics, and assertion work identical in both
/// full-corpus cases.
fn diagnose_all(world: &LspHarness, targets: &[FilePath], relative_paths: &[String]) {
    let per_file: Vec<(&str, Vec<Diagnostic>)> = targets
        .iter()
        .zip(relative_paths)
        .map(|(target, relative)| {
            let diagnostics = world.diagnose(target, world.snapshot()).unwrap();
            (relative.as_str(), diagnostics)
        })
        .collect();

    assert_all_diagnostics(&diagnostic_counts_by_path(&per_file));
}

/// Time `mutate` and the pass that follows it as two cases, so input-update
/// cost never hides inside the analysis number.
fn bench_mutation_then_pass(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixtures: &Fixtures,
    cache: &SourceCache,
    name: &str,
    mutation: &str,
    mutate: impl Fn(&mut LspHarness, &Fixtures),
) {
    group.bench_function(format!("{name}.{mutation}"), |bencher| {
        bencher.iter_batched_ref(
            || warm_setup(fixtures, cache),
            |(world, _target)| mutate(world, fixtures),
            BatchSize::PerIteration,
        );
    });

    group.bench_function(format!("{name}.pass"), |bencher| {
        bencher.iter_batched_ref(
            || {
                let (mut world, target) = warm_setup(fixtures, cache);
                mutate(&mut world, fixtures);
                (world, target)
            },
            |(world, target)| {
                let diagnostics = world.diagnose(target, world.snapshot()).unwrap();
                assert_diagnostics(&diagnostics);
            },
            BatchSize::PerIteration,
        );
    });
}

/// A database over the fixture corpus with the target open, before any pass.
fn setup(fixtures: &Fixtures, cache: &SourceCache) -> (LspHarness, FilePath) {
    match build_world(fixtures, cache) {
        Ok(world) => world,
        Err(err) => panic!("Failed to build the bench world: {err:?}"),
    }
}

/// Build the target database and prime the memos used by incremental cases.
fn warm_setup(fixtures: &Fixtures, cache: &SourceCache) -> (LspHarness, FilePath) {
    let (world, target) = setup(fixtures, cache);
    assert_diagnostics(&world.diagnose(&target, world.snapshot()).unwrap());
    (world, target)
}

fn setup_all(
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
) -> (LspHarness, Vec<FilePath>) {
    match build_world_all(fixtures, cache, files) {
        Ok(world) => world,
        Err(err) => panic!("Failed to build the bench world: {err:?}"),
    }
}

/// Runs one untimed full-corpus pass so [`bench_all_warm()`] measures warm
/// memos.
fn setup_all_warm(
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
    relative_paths: &[String],
) -> (LspHarness, Vec<FilePath>) {
    let (world, targets) = setup_all(fixtures, cache, files);
    diagnose_all(&world, &targets, relative_paths);
    (world, targets)
}

fn build_world(fixtures: &Fixtures, cache: &SourceCache) -> anyhow::Result<(LspHarness, FilePath)> {
    let mut world = build_world_base(fixtures, cache)?;

    let target = fixtures.workspace_path(TARGET);
    world.prepare_document(&target, read_source(&fixtures.dplyr().join(TARGET)))?;

    Ok((world, target))
}

/// Opens every file discovered by [`Fixtures::r_files()`] because
/// [`LspHarness::diagnose()`] only operates on open editor buffers.
fn build_world_all(
    fixtures: &Fixtures,
    cache: &SourceCache,
    files: &[PathBuf],
) -> anyhow::Result<(LspHarness, Vec<FilePath>)> {
    let mut world = build_world_base(fixtures, cache)?;

    let mut targets = Vec::with_capacity(files.len());
    for absolute in files {
        let target = fixtures.file_path(absolute.clone());
        world.prepare_document(&target, read_source(absolute))?;
        targets.push(target);
    }

    Ok((world, targets))
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
fn assert_all_diagnostics(counts: &[DiagnosticCountByPath<'_>]) {
    assert_eq!(counts, EXPECTED_ALL_DIAGNOSTIC_COUNTS);
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
