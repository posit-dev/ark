//! Generates deterministic scenarios from seeded `source()`-graph motifs.
//!
//! Each history toggles one edge. These are source cycles, not Salsa query
//! cycles. `Overlapping` has two cycles through file 0, so removing one edge
//! can leave the other intact.
//!
//! TODO(fuzz): This generator emits only `Source` and `Attach` effects, so the
//! rest of the fuzz vocabulary never reaches generated exploration. That
//! leaves out `Eval`, `Quote`, `QuoteHoles`, `Substitute`, `Assign`, `Rebind`,
//! and `SourceDir`, the last of which resolves against this materializer under
//! the `shallow_source_dir` and `recursive_source_dir_in_package` corpus
//! scenarios but is never generated. Pick rates from measured cost and
//! scenario diversity, since `sourceDir(".")` makes every script in a
//! workspace source every other one.

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use rand::rngs::StdRng;
use rand::RngExt;
use rand::SeedableRng;

use crate::file_imports::CollationView;
use crate::tests::fuzz::build::binding;
use crate::tests::fuzz::build::function_def;
use crate::tests::fuzz::build::library;
use crate::tests::fuzz::build::qualified_source;
use crate::tests::fuzz::build::shadow;
use crate::tests::fuzz::build::source;
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

/// Shape of the `source()` graph. Each history toggles one edge.
/// `Overlapping` remains cyclic after its toggled edge is removed.
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
    program: Program,
}

struct Edge {
    target: FileId,
    qualified: bool,
}

struct FileParts {
    path: String,
    attaches: Vec<String>,
    sources: Vec<Edge>,
    /// Callee to shadow locally. Never chosen from motif sources because that
    /// would remove an edge under test.
    shadow: Option<&'static str>,
    /// Supplies an identifier in deferred collation for offset-keyed queries.
    deferred: Option<String>,
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

        let mut parts: Vec<FileParts> = (0..count)
            .map(|index| FileParts::new(file_path(owner, index)))
            .collect();

        for (sourcing, target) in edges {
            parts[sourcing].sources.push(Edge {
                target: FileId(target),
                qualified: rng.random_bool(0.3),
            });
        }

        for part in &mut parts {
            if rng.random_bool(0.5) {
                part.attaches.push(attachable(rng, &installed));
            }
            if rng.random_bool(0.4) {
                part.deferred = Some(binding_name(rng.random_range(0..count)));
            }
        }

        let paths: Vec<String> = parts.iter().map(|part| part.path.clone()).collect();
        let mut files: Vec<FileDraft> = parts
            .iter()
            .enumerate()
            .map(|(index, part)| part.to_draft(index, &paths))
            .collect();

        // Exclude motif edges so shadowing cannot remove the edge under test.
        let shadowable: Vec<usize> = (0..count)
            .filter(|&index| index != closing.0 && parts[index].sources.is_empty())
            .collect();
        if !shadowable.is_empty() && rng.random_bool(0.3) {
            let index = *pick(rng, &shadowable);
            if let Some(callee) = pick_option(rng, &files[index].program.callees()) {
                parts[index].shadow = Some(callee.name);
                files[index] = parts[index].to_draft(index, &paths);
            }
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
                    program: file.program.clone(),
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
        let target_path = self.files[target.0].path.clone();
        let statements = &mut self.files[sourcing.0].program.statements;
        let existing = statements.iter().position(|stmt| {
            matches!(stmt, Stmt::Effect { recipe: EffectRecipe::Source { path, .. }, .. } if *path == target_path)
        });
        match existing {
            Some(position) => {
                statements.remove(position);
            },
            None => {
                let index = leading_calls_end(statements);
                statements.insert(index, source(&target_path));
            },
        }
        self.edit(sourcing)
    }

    /// Changes a binding without changing source edges or attached packages.
    fn touch(&mut self, rng: &mut StdRng) -> Op {
        let file = FileId(rng.random_range(0..self.files.len()));
        let statements = &mut self.files[file.0].program.statements;
        let next = statements
            .iter()
            .filter(|stmt| {
                matches!(stmt, Stmt::Bind {
                    value: Expr::Num(_),
                    ..
                })
            })
            .count();
        let touch = binding(&format!("touch_{}_{next}", file.0));
        match statements.iter().position(|stmt| {
            matches!(stmt, Stmt::Bind {
                value: Expr::Function { .. },
                ..
            })
        }) {
            Some(index) => statements.insert(index, touch),
            None => statements.push(touch),
        }
        self.edit(file)
    }

    fn edit(&self, file: FileId) -> Op {
        Op::Edit(Edit {
            file,
            program: self.files[file.0].program.clone(),
        })
    }
}

fn leading_calls_end(statements: &[Stmt]) -> usize {
    statements
        .iter()
        .position(|stmt| matches!(stmt, Stmt::Bind { .. }))
        .unwrap_or(statements.len())
}

impl FileParts {
    fn new(path: String) -> Self {
        FileParts {
            path,
            attaches: Vec::new(),
            sources: Vec::new(),
            shadow: None,
            deferred: None,
        }
    }

    fn to_draft(&self, index: usize, paths: &[String]) -> FileDraft {
        let mut statements = Vec::new();

        if let Some(name) = self.shadow {
            statements.push(shadow(name));
        }
        for package in &self.attaches {
            statements.push(library(package));
        }
        for edge in &self.sources {
            let target = &paths[edge.target.0];
            statements.push(if edge.qualified {
                qualified_source(target)
            } else {
                source(target)
            });
        }
        statements.push(binding(&binding_name(index)));
        if let Some(name) = &self.deferred {
            statements.push(function_def("read", vec![Stmt::use_of(name)]));
        }

        FileDraft {
            path: self.path.clone(),
            program: Program { statements },
        }
    }
}

fn file_path(owner: Owner, index: usize) -> String {
    let name = (b'a' + index as u8) as char;
    match owner {
        Owner::Script => format!("{name}.R"),
        Owner::Package => format!("R/{name}.R"),
    }
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

/// Enter each file-keyed `cycle_result` query directly. `Package::resolve()`
/// needs NAMESPACE re-exports, which these workspaces do not provide.
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

/// Run aggregates first to exercise their cold-entry cycle behavior.
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

/// Include `source` to query the shadowed-call case.
fn random_name(rng: &mut StdRng, spec: &WorkspaceSpec) -> String {
    if rng.random_bool(0.2) {
        return "source".to_string();
    }
    binding_name(rng.random_range(0..spec.files.len()))
}

fn pick<'items, T>(rng: &mut StdRng, items: &'items [T]) -> &'items T {
    &items[rng.random_range(0..items.len())]
}

fn pick_option<'items, T>(rng: &mut StdRng, items: &'items [T]) -> Option<&'items T> {
    if items.is_empty() {
        return None;
    }
    Some(&items[rng.random_range(0..items.len())])
}
