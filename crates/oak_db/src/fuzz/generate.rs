//! Builds fixed `source()`-graph motifs for mutation to grow from.
//!
//! These are source cycles, not Salsa query cycles. Mutation may redirect or
//! delete any edge, including one that makes a motif cyclic.
//!
//! Package re-export layers live in `packages`; draft assembly and edit-history
//! generation stay together here to preserve their random draw order.

mod packages;

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use rand::rngs::StdRng;
use rand::RngExt;
use rand::SeedableRng;

use self::packages::empty_package;
use self::packages::package_entry;
use self::packages::package_layer;
use self::packages::reexport_layer;
use self::packages::PackageLayer;
use self::packages::WORKSPACE_PACKAGE;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::qualified_source;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source;
use crate::fuzz::choose::binding_name;
use crate::fuzz::choose::cold_entries;
use crate::fuzz::choose::observing_query;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Shape;
use crate::fuzz::scenario::Edit;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::WorkspaceSpec;

const ATTACHABLE: [&str; 3] = ["pkga", "pkgb", "pkgc"];

const OBSERVE_EDIT_ODDS: f64 = 0.7;

/// Keep the smallest source-graph motifs flat, then add nested paths for
/// directory-walk coverage.
const NESTED_FROM: usize = 3;

const NESTED_DIR: &str = "sub";

/// Install every package named by [`EffectRecipe`] so mutated calls such as
/// `shiny::reactive()` and `library(S7)` can resolve.
pub(super) const EFFECT_PACKAGES: [&str; 4] = ["S7", "magrittr", "shiny", "targets"];

pub(super) const UNINSTALLED: &str = "pkgz";

pub(crate) fn seed_corpus(seed: u64) -> Vec<Scenario> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut scenarios = Vec::new();

    for (index, motif) in MOTIFS.into_iter().enumerate() {
        let layer = package_layer(seed, index);
        let mut draft = Draft::new(motif, layer, file_layout(seed, index), &mut rng);
        let initial = draft.spec();
        let ops = draft.history(&mut rng);

        // File 0 participates in every non-isolated motif; additional files
        // can be disconnected. Demand its index before any history edits.
        let mut entries = cold_entries(&mut rng, &Shape::of(&initial), FileId(0));

        // Pair the chain with a matching entry so package recovery coverage
        // does not depend on random query selection.
        entries.extend(draft.package_entry.clone());

        for cold_entry in entries {
            scenarios.push(Scenario {
                seed,
                variant: scenarios.len(),
                initial: initial.clone(),
                cold_entry,
                ops: ops.clone(),
            });
        }
    }

    scenarios
}

/// Shape of the `source()` graph. `Overlapping` has two cycles through file 0,
/// so dropping one edge can leave the other intact.
#[derive(Clone, Copy, Debug)]
enum Motif {
    /// No edges.
    Isolated,
    /// `0 -> 1 -> ... -> n-1`, acyclic.
    Chain,
    /// `0 -> 0`.
    SelfLoop,
    /// `0 -> 1 -> 0`.
    Mutual,
    /// `0 -> 1 -> 2 -> 0`.
    Ring,
    /// Two cycles sharing file 0.
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

/// Rotates testthat layouts by motif position so every seed corpus covers one
/// without tying it to a particular source graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileLayout {
    /// Loose scripts, or package collation with nested scripts under `inst/`.
    Plain,
    /// Package collation followed by `tests/testthat/` helpers and tests.
    Testthat,
}

/// One motif in three carries the testthat layout. A higher share would spend
/// most of the corpus on package-owned drafts, since testthat needs a package.
const TESTTHAT_PERIOD: usize = 3;

/// A testthat draft needs a collation member, helper, and test.
const TESTTHAT_FILES: usize = 3;

fn file_layout(seed: u64, motif: usize) -> FileLayout {
    let offset = (seed % TESTTHAT_PERIOD as u64) as usize;
    match (motif + offset) % TESTTHAT_PERIOD {
        0 => FileLayout::Testthat,
        _ => FileLayout::Plain,
    }
}

impl Motif {
    fn min_files(self) -> usize {
        match self {
            Motif::Isolated | Motif::SelfLoop => 1,
            Motif::Chain | Motif::Mutual => 2,
            Motif::Ring | Motif::Overlapping | Motif::Tail => 3,
        }
    }

    fn edges(self, files: usize) -> Vec<(usize, usize)> {
        match self {
            Motif::Isolated => vec![],
            Motif::Chain => (0..files - 1).map(|index| (index, index + 1)).collect(),
            Motif::SelfLoop => vec![(0, 0)],
            Motif::Mutual => vec![(0, 1), (1, 0)],
            Motif::Ring => vec![(0, 1), (1, 2), (2, 0)],
            Motif::Overlapping => vec![(0, 1), (1, 0), (0, 2), (2, 0)],
            Motif::Tail => vec![(0, 1), (1, 0), (2, 1)],
        }
    }
}

struct Draft {
    installed: Vec<String>,
    packages: Vec<PackageSpec>,
    owner: Owner,
    files: Vec<FileDraft>,
    /// Enters the re-export chain this draft built, `None` without one.
    package_entry: Option<Query>,
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
    shadow: Option<&'static str>,
    /// Supplies an identifier in deferred collation for offset-keyed queries.
    deferred: Option<String>,
    /// Supplies the local definition at the end of a re-export chain.
    local_export: Option<String>,
}

impl Draft {
    fn new(motif: Motif, layer: PackageLayer, layout: FileLayout, rng: &mut StdRng) -> Self {
        let drawn = rng.random_range(motif.min_files()..=MAX_FILES);
        let count = match layout {
            FileLayout::Testthat => drawn.max(TESTTHAT_FILES),
            FileLayout::Plain => drawn,
        };
        let edges = motif.edges(count);

        // `Local` and testthat layouts require a workspace package.
        let owner = if layer == PackageLayer::Local ||
            layout == FileLayout::Testthat ||
            rng.random_bool(0.5)
        {
            Owner::Package(PackageId(0))
        } else {
            Owner::Script
        };

        let mut installed = vec!["base".to_string()];
        installed.extend(EFFECT_PACKAGES.iter().map(|name| name.to_string()));
        // Lowering adds testthat's implicit attach only when it is installed.
        if layout == FileLayout::Testthat {
            installed.push("testthat".to_string());
        }
        for name in ATTACHABLE {
            if rng.random_bool(0.6) {
                installed.push(name.to_string());
            }
        }

        let mut parts: Vec<FileParts> = (0..count)
            .map(|index| FileParts::new(layout_path(layout, owner, index)))
            .collect();

        for (sourcing, target) in edges {
            parts[sourcing].sources.push(Edge {
                target: FileId(target),
                qualified: rng.random_bool(0.3),
            });
        }

        let (workspace_package, reexport_libs) = reexport_layer(rng, layer, &mut parts);
        let mut packages = Vec::new();
        match owner {
            Owner::Package(_) => {
                packages.push(workspace_package.unwrap_or_else(|| empty_package(WORKSPACE_PACKAGE)))
            },
            Owner::Script => {},
        }
        packages.extend(reexport_libs);

        let attachable_names: Vec<String> = installed
            .iter()
            .cloned()
            .chain(packages.iter().map(|package| package.name.clone()))
            .collect();
        for part in &mut parts {
            if rng.random_bool(0.5) {
                part.attaches.push(attachable(rng, &attachable_names));
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

        let index = rng.random_range(0..count);
        if rng.random_bool(0.3) {
            if let Some(callee) = pick_option(rng, &files[index].program.callees()) {
                parts[index].shadow = Some(callee.name);
                files[index] = parts[index].to_draft(index, &paths);
            }
        }

        Self {
            package_entry: package_entry(layer, owner),
            installed,
            packages,
            owner,
            files,
        }
    }

    fn spec(&self) -> WorkspaceSpec {
        WorkspaceSpec {
            installed: self.installed.clone(),
            packages: self.packages.clone(),
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
        // Edits change programs, never the file or package lists the queries
        // draw from, so one shape serves the whole history.
        let shape = Shape::of(&self.spec());

        for _ in 0..rng.random_range(1..=3) {
            // Preserve unrelated entry points instead of making every query follow an edit.
            ops.push(Op::Query(random_query(rng, &shape)));
            if rng.random_bool(0.3) {
                // Keep consecutive, unobserved replacements reachable.
                let touch = self.touch(rng);
                ops.push(touch);
                ops.push(Op::Query(random_query(rng, &shape)));
            }
            let edit = self.toggle_edge(rng);
            let edited = edit.file();
            ops.push(edit);
            ops.push(self.query_after_edit(rng, &shape, edited));
        }

        ops
    }

    /// Usually targets the edited file, while random draws preserve unrelated
    /// entry points. Inferring true consumers would duplicate effect resolution,
    /// including shadow and provider suppression. Source motifs still exercise
    /// cross-file paths.
    fn query_after_edit(&self, rng: &mut StdRng, shape: &Shape, edited: Option<FileId>) -> Op {
        let Some(edited) = edited else {
            return Op::Query(random_query(rng, shape));
        };
        if !rng.random_bool(OBSERVE_EDIT_ODDS) {
            return Op::Query(random_query(rng, shape));
        }
        Op::Query(observing_query(rng, shape, edited))
    }

    fn toggle_edge(&mut self, rng: &mut StdRng) -> Op {
        let sourcing = FileId(rng.random_range(0..self.files.len()));
        let target_path = self.files[rng.random_range(0..self.files.len())]
            .path
            .clone();
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
            local_export: None,
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
        if let Some(name) = &self.local_export {
            statements.push(function_def(name, vec![]));
        }
        if let Some(name) = &self.deferred {
            statements.push(function_def("read", vec![Stmt::use_of(name)]));
        }

        FileDraft {
            path: self.path.clone(),
            program: Program { statements },
        }
    }
}

/// A testthat draft keeps its first file in `R/` for package collation, then
/// alternates helpers and tests. testthat loads all helpers before each test,
/// exercising the support prefix that `CollationView` narrows.
fn layout_path(layout: FileLayout, owner: Owner, index: usize) -> String {
    let name = (b'a' + index as u8) as char;
    match (layout, index) {
        (FileLayout::Plain, _) | (FileLayout::Testthat, 0) => file_path(owner, index),
        (FileLayout::Testthat, _) if index % 2 == 1 => {
            format!("tests/testthat/helper-{name}.R")
        },
        (FileLayout::Testthat, _) => format!("tests/testthat/test-{name}.R"),
    }
}

/// Places later files in a subdirectory so shallow and recursive walks differ.
/// Deriving paths from `index` lets `add_file()` probe for an unused path.
///
/// Nested package files use `inst/`, because R loads only direct `R/` children.
/// [`classify_in_package()`] skips nested `R/` files, while directory walks can
/// still reach `inst/` scripts.
///
/// [`classify_in_package()`]: crate::classify_in_package
pub(super) fn file_path(owner: Owner, index: usize) -> String {
    let name = (b'a' + index as u8) as char;
    match owner {
        Owner::Script if index >= NESTED_FROM => format!("{NESTED_DIR}/{name}.R"),
        Owner::Script => format!("{name}.R"),
        Owner::Package(_) if index >= NESTED_FROM => format!("inst/{NESTED_DIR}/{name}.R"),
        Owner::Package(_) => format!("R/{name}.R"),
    }
}

fn attachable(rng: &mut StdRng, installed: &[String]) -> String {
    let candidates: Vec<&String> = installed.iter().filter(|name| *name != "base").collect();
    if candidates.is_empty() || rng.random_bool(0.15) {
        return UNINSTALLED.to_string();
    }
    (*pick(rng, &candidates)).clone()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::choose::observed_file;

    #[test]
    fn test_seed_corpus_observes_source_motifs() {
        for motif in MOTIFS {
            for files in motif.min_files()..=MAX_FILES {
                let edges = motif.edges(files);
                if !matches!(motif, Motif::Isolated) {
                    assert!(edges.iter().any(|&(from, _)| from == 0));
                }
            }
        }

        for seed in 0..6 {
            let corpus = seed_corpus(seed);
            let mut start = 0;
            for index in 0..MOTIFS.len() {
                assert_eq!(
                    observed_file(&corpus[start + 2].cold_entry),
                    Some(FileId(0))
                );
                start += 4 + usize::from(package_layer(seed, index) != PackageLayer::Bare);
            }
            assert_eq!(start, corpus.len());
        }
    }
}
