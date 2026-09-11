//! Generates concrete scenarios from seeded source-graph motifs.
//!
//! Each seed selects a `source()` graph, renders its files as R code, and
//! generates queries around one to three edits that toggle one source edge.
//! For example, adding `b.R -> a.R` closes the cycle `a.R -> b.R -> a.R`.
//! These are source cycles, not Salsa query cycles. The `Overlapping` motif has
//! two source cycles sharing file 0, so removing one edge may leave the other
//! cycle intact.
//!
//! Randomness is confined here. The resulting [`Scenario`] contains the exact
//! workspace and operation history needed for deterministic execution and
//! failure reporting.

use rand::rngs::StdRng;
use rand::RngExt;
use rand::SeedableRng;

use crate::file_imports::CollationView;
use crate::tests::fuzz::scenario::Edit;
use crate::tests::fuzz::scenario::Op;
use crate::tests::fuzz::scenario::Query;
use crate::tests::fuzz::scenario::Scenario;
use crate::tests::fuzz::scenario::Site;
use crate::tests::fuzz::spec::FileId;
use crate::tests::fuzz::spec::FileSpec;
use crate::tests::fuzz::spec::Owner;
use crate::tests::fuzz::spec::WorkspaceSpec;

const ATTACHABLE: [&str; 3] = ["pkga", "pkgb", "pkgc"];

const UNINSTALLED: &str = "pkgz";

const MAX_FILES: usize = 4;

/// Use a fresh database for each first query because Salsa's repeated key
/// depends on entry order.
pub(super) fn generate(seed: u64) -> Vec<Scenario> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut draft = Draft::new(&mut rng);
    let initial = draft.spec();
    let ops = draft.history(&mut rng);

    cold_entries(&mut rng, &initial)
        .into_iter()
        .enumerate()
        .map(|(variant, cold_entry)| Scenario {
            seed,
            variant,
            initial: initial.clone(),
            cold_entry,
            ops: ops.clone(),
        })
        .collect()
}

/// Shape of the `source()` graph. The history toggles one designated edge,
/// changing cycle status except for `Overlapping`, which retains a second cycle
/// after that edge is removed.
#[derive(Clone, Copy, Debug)]
enum Motif {
    /// No edges. The toggle adds a self-loop.
    Isolated,
    /// `0 -> 1 -> ... -> n-1`, acyclic. The toggle closes the ring.
    Chain,
    /// `0 -> 0`.
    SelfLoop,
    /// `0 -> 1 -> 0`.
    Mutual,
    /// `0 -> 1 -> 2 -> 0`.
    Ring,
    /// Two cycles sharing file 0. The toggle leaves one of them standing.
    Overlapping,
    /// `0 <-> 1` with `2 -> 1` hanging off it.
    Tail,
}

const MOTIFS: [Motif; 7] = [
    Motif::Isolated,
    Motif::Chain,
    Motif::SelfLoop,
    Motif::Mutual,
    Motif::Ring,
    Motif::Overlapping,
    Motif::Tail,
];

impl Motif {
    fn min_files(self) -> usize {
        match self {
            Motif::Isolated | Motif::SelfLoop => 1,
            Motif::Chain | Motif::Mutual => 2,
            Motif::Ring | Motif::Overlapping | Motif::Tail => 3,
        }
    }

    fn edges(self, files: usize) -> (Vec<(usize, usize)>, (usize, usize)) {
        match self {
            Motif::Isolated => (vec![], (0, 0)),
            Motif::Chain => {
                let edges = (0..files - 1).map(|index| (index, index + 1)).collect();
                (edges, (files - 1, 0))
            },
            Motif::SelfLoop => (vec![(0, 0)], (0, 0)),
            Motif::Mutual => (vec![(0, 1), (1, 0)], (1, 0)),
            Motif::Ring => (vec![(0, 1), (1, 2), (2, 0)], (2, 0)),
            Motif::Overlapping => (vec![(0, 1), (1, 0), (0, 2), (2, 0)], (2, 0)),
            Motif::Tail => (vec![(0, 1), (1, 0), (2, 1)], (1, 0)),
        }
    }
}

struct Draft {
    installed: Vec<String>,
    package: Option<String>,
    owner: Owner,
    files: Vec<FileDraft>,
    closing: (FileId, FileId),
}

struct FileDraft {
    path: String,
    attaches: Vec<String>,
    sources: Vec<Edge>,
    bindings: Vec<String>,
    /// Restrict shadowing to files without out-edges so it cannot remove an edge
    /// required by the selected motif.
    shadowed_call: Option<String>,
    /// Keep a deferred identifier available to offset-keyed queries.
    deferred: Option<String>,
}

struct Edge {
    target: FileId,
    qualified: bool,
}

impl Draft {
    fn new(rng: &mut StdRng) -> Self {
        let motif = *pick(rng, &MOTIFS);
        let count = rng.random_range(motif.min_files()..=MAX_FILES);
        let (edges, closing) = motif.edges(count);

        let owner = if rng.random_bool(0.5) {
            Owner::Script
        } else {
            Owner::Package
        };

        let mut installed = vec!["base".to_string()];
        for name in ATTACHABLE {
            if rng.random_bool(0.6) {
                installed.push(name.to_string());
            }
        }

        let mut files: Vec<FileDraft> = (0..count)
            .map(|index| FileDraft {
                path: file_path(owner, index),
                attaches: Vec::new(),
                sources: Vec::new(),
                bindings: vec![binding(index)],
                shadowed_call: None,
                deferred: None,
            })
            .collect();

        for (sourcing, target) in edges {
            files[sourcing].sources.push(Edge {
                target: FileId(target),
                qualified: rng.random_bool(0.3),
            });
        }

        for file in &mut files {
            if rng.random_bool(0.5) {
                file.attaches.push(attachable(rng, &installed));
            }
            if rng.random_bool(0.4) {
                file.deferred = Some(binding_name(rng.random_range(0..count)));
            }
        }

        // Avoid files with current or future motif edges because shadowing
        // `source()` there could remove the edge that closes the cycle.
        let shadowable: Vec<usize> = (0..count)
            .filter(|&index| index != closing.0 && files[index].sources.is_empty())
            .collect();
        if !shadowable.is_empty() && rng.random_bool(0.3) {
            let index = *pick(rng, &shadowable);
            let target = rng.random_range(0..count);
            files[index].shadowed_call = Some(files[target].path.clone());
        }

        Self {
            installed,
            package: match owner {
                Owner::Script => None,
                Owner::Package => Some("mypkg".to_string()),
            },
            owner,
            files,
            closing: (FileId(closing.0), FileId(closing.1)),
        }
    }

    fn spec(&self) -> WorkspaceSpec {
        WorkspaceSpec {
            installed: self.installed.clone(),
            package: self.package.clone(),
            files: self
                .files
                .iter()
                .map(|file| FileSpec {
                    owner: self.owner,
                    path: file.path.clone(),
                    contents: self.render(file),
                })
                .collect(),
        }
    }

    fn history(&mut self, rng: &mut StdRng) -> Vec<Op> {
        let mut ops = Vec::new();
        let spec = self.spec();

        for _ in 0..rng.random_range(1..=3) {
            ops.push(Op::Query(random_query(rng, &spec)));
            if rng.random_bool(0.3) {
                ops.push(self.touch(rng));
                ops.push(Op::Query(random_query(rng, &spec)));
            }
            ops.push(self.toggle_closing());
            ops.push(Op::Query(random_query(rng, &spec)));
        }

        ops
    }

    fn toggle_closing(&mut self) -> Op {
        let (sourcing, target) = self.closing;
        let sources = &mut self.files[sourcing.0].sources;
        match sources.iter().position(|edge| edge.target == target) {
            Some(position) => {
                sources.remove(position);
            },
            None => sources.push(Edge {
                target,
                qualified: false,
            }),
        }
        self.edit(sourcing)
    }

    /// Preserve edges and attaches so some invalidations are unrelated to cycle
    /// structure.
    fn touch(&mut self, rng: &mut StdRng) -> Op {
        let file = FileId(rng.random_range(0..self.files.len()));
        let next = self.files[file.0].bindings.len();
        self.files[file.0]
            .bindings
            .push(format!("touch_{}_{next} <- 1", file.0));
        self.edit(file)
    }

    fn edit(&self, file: FileId) -> Op {
        Op::Edit(Edit {
            file,
            contents: self.render(&self.files[file.0]),
        })
    }

    fn render(&self, file: &FileDraft) -> String {
        let mut out = String::new();
        if let Some(target) = &file.shadowed_call {
            out.push_str("source <- function(...) NULL\n");
            out.push_str(&format!("source(\"{target}\")\n"));
        }
        for package in &file.attaches {
            out.push_str(&format!("library({package})\n"));
        }
        for edge in &file.sources {
            let callee = if edge.qualified {
                "base::source"
            } else {
                "source"
            };
            let target = &self.files[edge.target.0].path;
            out.push_str(&format!("{callee}(\"{target}\")\n"));
        }
        for binding in &file.bindings {
            out.push_str(&format!("{binding}\n"));
        }
        if let Some(name) = &file.deferred {
            out.push_str(&format!("read <- function() {name}\n"));
        }
        out
    }
}

fn file_path(owner: Owner, index: usize) -> String {
    let name = (b'a' + index as u8) as char;
    match owner {
        Owner::Script => format!("{name}.R"),
        Owner::Package => format!("R/{name}.R"),
    }
}

fn binding(index: usize) -> String {
    format!("{} <- 1", binding_name(index))
}

fn binding_name(index: usize) -> String {
    format!("val_{index}")
}

fn attachable(rng: &mut StdRng, installed: &[String]) -> String {
    let candidates: Vec<&String> = installed.iter().filter(|name| *name != "base").collect();
    if candidates.is_empty() || rng.random_bool(0.15) {
        return UNINSTALLED.to_string();
    }
    (*pick(rng, &candidates)).clone()
}

/// Give each entry query a fresh database because Salsa's repeated key depends
/// on entry order.
fn cold_entries(rng: &mut StdRng, spec: &WorkspaceSpec) -> Vec<Query> {
    vec![
        cycle_entry(rng, spec),
        aggregate_entry(rng),
        production_entry(rng, spec),
        random_query(rng, spec),
    ]
}

/// Enter each file-keyed `cycle_result` query directly. `Package::resolve()` is
/// excluded because the generated workspaces have no NAMESPACE re-exports.
fn cycle_entry(rng: &mut StdRng, spec: &WorkspaceSpec) -> Query {
    let file = random_file(rng, spec);
    let view = random_view(rng);
    match rng.random_range(0..6) {
        0 => Query::SemanticIndex(file),
        1 => Query::Exports(file),
        2 => Query::AttachedPackages(file),
        3 => Query::AttachedPackagesAnywhere(file),
        4 => Query::InheritedLayers(file, view),
        _ => Query::CrossFileLayers(file, view),
    }
}

/// Isolate each aggregate so it can be the first query Salsa enters.
fn aggregate_entry(rng: &mut StdRng) -> Query {
    match rng.random_range(0..5) {
        0 => Query::AllPackageDependencies,
        1 => Query::AllWorkspaceFileDependencies,
        2 => Query::AllWorkspaceLoaderDependencies,
        3 => Query::AllWorkspacePackageDependencies,
        _ => Query::DefaultSearchPathPackages,
    }
}

fn production_entry(rng: &mut StdRng, spec: &WorkspaceSpec) -> Query {
    let file = random_file(rng, spec);
    match rng.random_range(0..7) {
        0 => Query::Diagnostics(file),
        1 => Query::Imports(file),
        2 => Query::ImportsAt(file, random_site(rng)),
        3 => Query::ResolveAt(file, random_site(rng)),
        4 => Query::Resolve(file, random_name(rng, spec)),
        5 => Query::UsedPackages(file),
        _ => Query::SourcedBy(file),
    }
}

fn random_query(rng: &mut StdRng, spec: &WorkspaceSpec) -> Query {
    match rng.random_range(0..3) {
        0 => production_entry(rng, spec),
        1 => aggregate_entry(rng),
        _ => cycle_entry(rng, spec),
    }
}

fn random_file(rng: &mut StdRng, spec: &WorkspaceSpec) -> FileId {
    FileId(rng.random_range(0..spec.files.len()))
}

fn random_site(rng: &mut StdRng) -> Site {
    match rng.random_range(0..3) {
        0 => Site::FirstCall,
        1 => Site::LastIdentifier,
        _ => Site::Eof,
    }
}

fn random_view(rng: &mut StdRng) -> CollationView {
    if rng.random_bool(0.5) {
        CollationView::Eager
    } else {
        CollationView::Deferred
    }
}

/// A name a file might bind, plus `source` so the shadowing case is queried.
fn random_name(rng: &mut StdRng, spec: &WorkspaceSpec) -> String {
    if rng.random_bool(0.2) {
        return "source".to_string();
    }
    binding_name(rng.random_range(0..spec.files.len()))
}

fn pick<'items, T>(rng: &mut StdRng, items: &'items [T]) -> &'items T {
    &items[rng.random_range(0..items.len())]
}
