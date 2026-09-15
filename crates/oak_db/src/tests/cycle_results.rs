//! Query results after cycle recovery.
//!
//! [`tests::recovery`] pins which handlers fire. These tests pin some of the
//! values the handlers leave behind.
//!
//! Local definitions survive the `semantic_index()` fallback, which rebuilds
//! with `NoopImportsResolver`. The separate `exports()` fallback returns empty.
//! In these fixtures, `resolve_at()` therefore finds the tested local binding
//! and use, while end-of-file `resolve()` misses the same name through exports.
//! These assertions do not cover cursor resolution of imported names.
//!
//! [`tests::recovery`]: crate::tests::recovery

use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::file_imports::install_packages;
use crate::tests::file_imports::shape;
use crate::tests::test_db::file_path;
use crate::tests::test_db::make_package;
use crate::tests::test_db::path_name;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::DiagnosticKind;
use crate::File;
use crate::FileRevision;
use crate::Name;

struct Probe {
    db: TestDb,
    files: Vec<File>,
}

fn build(packages: &[&str], root_path: &str, scripts: &[(&str, &str)]) -> Probe {
    let mut db = TestDb::new();
    install_packages(&mut db, packages);
    let root = workspace_root(&db, root_path);
    let files: Vec<File> = scripts
        .iter()
        .map(|(path, text)| {
            File::new(
                &db,
                file_path(path),
                FileRevision::zero(),
                Some(text.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(&mut db).to(files.clone());
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    Probe { db, files }
}

const PAIR_A: &str = "source(\"b.R\")\na_local <- 1\na_local\n";

/// Explicit mutual sourcing keeps this cycle independent of directory
/// collation. `c.R` neither sources another file nor is sourced by one.
fn mutual_pair() -> Probe {
    build(&["base"], "w", &[
        ("w/a.R", PAIR_A),
        ("w/b.R", "source(\"a.R\")\nb_local <- 2\n"),
        ("w/c.R", "c_local <- 3\n"),
    ])
}

/// Differs from `mutual_pair()` only by removing `b.R`'s back edge.
fn mutual_pair_no_cycle() -> Probe {
    build(&["base"], "w", &[
        ("w/a.R", PAIR_A),
        ("w/b.R", "b_local <- 2\n"),
        ("w/c.R", "c_local <- 3\n"),
    ])
}

/// The #15631 cycle needs only one source edge between collation siblings.
/// Collation supplies the return dependency. `c.R` shares the directory but
/// stays outside the cycle. Package collation stands in for the loose-script
/// `R/` fallback the original regression hit, since that fallback is gone.
fn collation_ring() -> Probe {
    let mut db = TestDb::new();
    install_packages(&mut db, &["pkga", "pkgb", "pkgc"]);
    let (pkg, files) = make_package(&mut db, "proj", Namespace::default(), &[
        (
            "ws/proj/R/a.R",
            "library(pkga)\nsource(\"R/b.R\")\na_local <- 1\n",
        ),
        ("ws/proj/R/b.R", "library(pkgb)\nb_local <- 2\n"),
        ("ws/proj/R/c.R", "library(pkgc)\nc_local <- 3\n"),
    ]);
    let root = workspace_root(&db, "ws/proj");
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    Probe { db, files }
}

/// Without a cycle, `c.R` inherits `a.R`'s shadow of `source()` and suppresses
/// the effect of its later call. Only `b.R` differs between the two fixtures.
fn shadow_consumer(b_text: &str) -> Probe {
    build(&["base"], "w", &[
        ("w/a.R", "source(\"b.R\")\nsource <- function(...) NULL\n"),
        ("w/b.R", b_text),
        ("w/c.R", "source(\"a.R\")\nsource(\"target.R\")\n"),
        ("w/target.R", "target_local <- 1\n"),
    ])
}

fn shadow_consumer_cycling() -> Probe {
    shadow_consumer("source(\"a.R\")\n")
}

fn shadow_consumer_no_cycle() -> Probe {
    shadow_consumer("b_local <- 1\n")
}

/// The query that runs before anything else on a fresh database.
#[derive(Clone, Copy, Debug)]
enum Entry {
    Index,
    Exports,
    Imports,
    Diagnostics,
    UsedPackages,
    SourcedBy,
    AttachedPackages,
}

const ENTRIES: [Entry; 7] = [
    Entry::Index,
    Entry::Exports,
    Entry::Imports,
    Entry::Diagnostics,
    Entry::UsedPackages,
    Entry::SourcedBy,
    Entry::AttachedPackages,
];

/// Compares export names and diagnostic kinds, not export payloads or diagnostic
/// spans. Offset-based queries, `resolve()`, `used_packages()`, package resolution,
/// and recovery firing sets are excluded.
#[derive(Clone, Copy, Debug)]
enum Observable {
    Exports,
    Imports,
    Attached,
    SourcedBy,
    Diagnostics,
}

const OBSERVABLES: [Observable; 5] = [
    Observable::Exports,
    Observable::Imports,
    Observable::Attached,
    Observable::SourcedBy,
    Observable::Diagnostics,
];

impl Probe {
    fn enter(&self, entry: Entry, file: File) {
        let db = &self.db;
        match entry {
            Entry::Index => drop(file.semantic_index(db)),
            Entry::Exports => drop(file.exports(db)),
            Entry::Imports => drop(file.imports(db)),
            Entry::Diagnostics => drop(file.diagnostics(db)),
            Entry::UsedPackages => drop(file.used_packages(db)),
            Entry::SourcedBy => drop(file.sourced_by(db)),
            Entry::AttachedPackages => drop(file.attached_packages(db)),
        }
    }

    fn read(&self, observable: Observable, file: File) -> String {
        match observable {
            Observable::Exports => format!("{:?}", self.exports(file)),
            Observable::Imports => format!("{:?}", shape(&self.db, file.imports(&self.db))),
            Observable::Attached => format!("{:?}", self.attached(file)),
            Observable::SourcedBy => format!("{:?}", self.sourced_by(file)),
            Observable::Diagnostics => format!("{:?}", self.diagnostics(file)),
        }
    }

    fn exports(&self, file: File) -> Vec<String> {
        let mut names: Vec<String> = file
            .exports(&self.db)
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        names.sort();
        names
    }

    fn attached(&self, file: File) -> Vec<String> {
        file.attached_packages(&self.db)
            .iter()
            .map(|package| package.text(&self.db).to_string())
            .collect()
    }

    /// End-of-file resolution, which consults the exports chain first.
    fn resolve(&self, file: File, name: &str) -> usize {
        file.resolve(&self.db, Name::new(&self.db, name)).len()
    }

    /// Cursor resolution at the byte offset of `needle`'s last occurrence.
    fn resolve_at(&self, file: File, text: &str, needle: &str) -> usize {
        let offset = text.rfind(needle).unwrap();
        file.resolve_at(&self.db, TextSize::from(offset as u32))
            .len()
    }

    fn sourced_by(&self, file: File) -> Vec<String> {
        file.sourced_by(&self.db)
            .iter()
            .map(|sourcing| path_name(sourcing.path(&self.db)))
            .collect()
    }

    fn diagnostics(&self, file: File) -> Vec<String> {
        let mut kinds: Vec<String> = file
            .diagnostics(&self.db)
            .iter()
            .map(|diagnostic| diagnostic.kind().as_str().to_string())
            .collect();
        kinds.sort();
        kinds
    }

    fn cycle_participants(&self) -> usize {
        self.files
            .iter()
            .filter(|&&file| {
                self.diagnostics(file)
                    .iter()
                    .any(|kind| kind == DiagnosticKind::SourceCycle.as_str())
            })
            .count()
    }
}

/// Each observation gets a fresh database with only the selected entry request
/// run beforehand. Otherwise, earlier observations could warm the cache and
/// hide differences caused by the entry order.
fn read_after(
    make: fn() -> Probe,
    entry: Entry,
    entry_file: usize,
    observable: Observable,
    target: usize,
) -> String {
    let probe = make();
    probe.enter(entry, probe.files[entry_file]);
    probe.read(observable, probe.files[target])
}

/// Uses the first entry on the first file as the baseline for each observation.
fn divergences(make: fn() -> Probe) -> Vec<String> {
    let count = make().files.len();
    let mut diverging = Vec::new();
    for observable in OBSERVABLES {
        for target in 0..count {
            let baseline = read_after(make, ENTRIES[0], 0, observable, target);
            for entry in ENTRIES {
                for entry_file in 0..count {
                    if read_after(make, entry, entry_file, observable, target) != baseline {
                        diverging.push(format!(
                            "{observable:?}[{target}] after {entry:?} on [{entry_file}]"
                        ));
                    }
                }
            }
        }
    }
    diverging
}

/// Checks diagnostics on a separate database to keep the entry-order tests cold.
fn assert_fixture_cycles(make: fn() -> Probe) {
    assert!(make().cycle_participants() > 0);
}

fn assert_fixture_does_not_cycle(make: fn() -> Probe) {
    assert_eq!(make().cycle_participants(), 0);
}

// == Fixtures still exercise recovery ==
//
// Check both sides of each comparison. Removing a cycle could make entry-order
// agreement vacuous, while adding one to a control would invalidate the comparison.

#[test]
fn test_mutual_pair_cycles_only_with_the_back_edge() {
    assert_fixture_cycles(mutual_pair);
    assert_fixture_does_not_cycle(mutual_pair_no_cycle);
}

#[test]
fn test_collation_ring_cycles() {
    assert_fixture_cycles(collation_ring);
}

#[test]
fn test_shadow_consumer_cycles_only_with_the_back_edge() {
    assert_fixture_cycles(shadow_consumer_cycling);
    assert_fixture_does_not_cycle(shadow_consumer_no_cycle);
}

// == Entry order ==
//
// Salsa's repeated key depends on which query entered the cycle, and the
// recovery log cannot name that key. These tests establish agreement for the
// `OBSERVABLES` over the `ENTRIES`, on these fixtures. They say nothing about
// other queries or other cyclic workspaces.

#[test]
fn test_mutual_pair_observables_agree_across_these_cold_entries() {
    assert_eq!(divergences(mutual_pair), Vec::<String>::new());
}

#[test]
fn test_collation_ring_observables_agree_across_these_cold_entries() {
    assert_eq!(divergences(collation_ring), Vec::<String>::new());
}

// == What a participant loses ==

#[test]
fn test_cycle_participant_exports_nothing_it_defines() {
    let probe = mutual_pair();
    let (a, b, c) = (probe.files[0], probe.files[1], probe.files[2]);

    // `a_local <- 1` and `b_local <- 2` are ordinary top-level assignments
    // with no part in the cycle.
    assert_eq!(probe.exports(a), Vec::<String>::new());
    assert_eq!(probe.exports(b), Vec::<String>::new());
    assert_eq!(probe.exports(c), vec!["c_local".to_string()]);
}

#[test]
fn test_cycle_participant_still_resolves_its_own_binding_at_the_cursor() {
    // Rebuilding in `semantic_index()` recovery preserves the local use-def map.
    let cycling = mutual_pair();
    assert_eq!(cycling.resolve_at(cycling.files[0], PAIR_A, "a_local"), 1);
    assert_eq!(
        cycling.resolve_at(cycling.files[0], PAIR_A, "a_local <-"),
        1
    );

    let control = mutual_pair_no_cycle();
    assert_eq!(control.resolve_at(control.files[0], PAIR_A, "a_local"), 1);
}

#[test]
fn test_cycle_participant_loses_end_of_file_resolution_through_exports() {
    // The empty `exports()` fallback hides this local name from `resolve()`,
    // even though it remains in the semantic index.
    let cycling = mutual_pair();
    assert_eq!(cycling.resolve(cycling.files[0], "a_local"), 0);

    let control = mutual_pair_no_cycle();
    assert_eq!(control.resolve(control.files[0], "a_local"), 1);
}

#[test]
fn test_collation_ring_participants_lose_their_own_attaches() {
    let probe = collation_ring();
    let (a, b, c) = (probe.files[0], probe.files[1], probe.files[2]);

    assert_eq!(probe.attached(a), Vec::<String>::new());
    assert_eq!(probe.attached(b), Vec::<String>::new());
    assert_eq!(probe.attached(c), vec!["pkgc".to_string()]);

    assert_eq!(probe.exports(a), Vec::<String>::new());
    assert_eq!(probe.exports(c), vec!["c_local".to_string()]);
}

// == Degraded shadowing ==
//
// Losing `a.R`'s exports exposes `base::source()` to its consumer, `c.R`.
// The consumer gains a source edge without receiving a cycle diagnostic.
//
// This characterizes recovery without prescribing the desired result. The
// cyclic program recurses before reaching the shadowing assignment, so runtime
// execution does not establish whether analysis should retain that edge.

#[test]
fn test_inherited_shadow_suppresses_the_consumers_effect() {
    let probe = shadow_consumer_no_cycle();
    let (a, target) = (probe.files[0], probe.files[3]);

    // `a.R` forwards `b.R`'s exports alongside its own binding.
    assert_eq!(probe.exports(a), vec![
        "b_local".to_string(),
        "source".to_string()
    ]);

    // `c.R` inherits the shadow, so its `source("target.R")` is not an effect
    // and no edge to `target.R` forms.
    assert_eq!(probe.sourced_by(target), Vec::<String>::new());
    assert_eq!(probe.sourced_by(a), vec!["w/c.R".to_string()]);
}

#[test]
fn test_cycle_erases_the_shadow_and_the_consumer_recognizes_the_effect() {
    let probe = shadow_consumer_cycling();
    let (a, c, target) = (probe.files[0], probe.files[2], probe.files[3]);

    assert_eq!(probe.exports(a), Vec::<String>::new());

    assert_eq!(probe.diagnostics(c), Vec::<String>::new());
    assert_eq!(probe.sourced_by(target), vec!["w/c.R".to_string()]);
}
