//! Builds fixed `source()`-graph motifs for mutation to grow from.
//!
//! These are source cycles, not Salsa query cycles. Mutation may redirect or
//! delete any edge, including one that makes a motif cyclic.

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use rand::rngs::StdRng;
use rand::RngExt;
use rand::SeedableRng;

use crate::tests::fuzz::build::binding;
use crate::tests::fuzz::build::function_def;
use crate::tests::fuzz::build::library;
use crate::tests::fuzz::build::qualified_source;
use crate::tests::fuzz::build::shadow;
use crate::tests::fuzz::build::source;
use crate::tests::fuzz::choose::binding_name;
use crate::tests::fuzz::choose::cold_entries;
use crate::tests::fuzz::choose::random_query;
use crate::tests::fuzz::mutate::MAX_FILES;
use crate::tests::fuzz::scenario::Edit;
use crate::tests::fuzz::scenario::Op;
use crate::tests::fuzz::scenario::Scenario;
use crate::tests::fuzz::spec::FileId;
use crate::tests::fuzz::spec::FileSpec;
use crate::tests::fuzz::spec::Owner;
use crate::tests::fuzz::spec::WorkspaceSpec;

const ATTACHABLE: [&str; 3] = ["pkga", "pkgb", "pkgc"];

/// Install every package named by [`EffectRecipe`] so mutated calls such as
/// `shiny::reactive()` and `library(S7)` can resolve.
pub(super) const EFFECT_PACKAGES: [&str; 4] = ["S7", "magrittr", "shiny", "targets"];

pub(super) const UNINSTALLED: &str = "pkgz";

pub(super) fn seed_corpus(seed: u64) -> Vec<Scenario> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut scenarios = Vec::new();

    for motif in MOTIFS {
        let mut draft = Draft::new(motif, &mut rng);
        let initial = draft.spec();
        let ops = draft.history(&mut rng);

        for cold_entry in cold_entries(&mut rng, initial.files.len()) {
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
    package: Option<String>,
    owner: Owner,
    files: Vec<FileDraft>,
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
}

impl Draft {
    fn new(motif: Motif, rng: &mut StdRng) -> Self {
        let count = rng.random_range(motif.min_files()..=MAX_FILES);
        let edges = motif.edges(count);

        let owner = if rng.random_bool(0.5) {
            Owner::Script
        } else {
            Owner::Package
        };

        let mut installed = vec!["base".to_string()];
        installed.extend(EFFECT_PACKAGES.iter().map(|name| name.to_string()));
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

        let index = rng.random_range(0..count);
        if rng.random_bool(0.3) {
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
        let files = self.files.len();

        for _ in 0..rng.random_range(1..=3) {
            ops.push(Op::Query(random_query(rng, files)));
            if rng.random_bool(0.3) {
                ops.push(self.touch(rng));
                ops.push(Op::Query(random_query(rng, files)));
            }
            ops.push(self.toggle_edge(rng));
            ops.push(Op::Query(random_query(rng, files)));
        }

        ops
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

pub(super) fn file_path(owner: Owner, index: usize) -> String {
    let name = (b'a' + index as u8) as char;
    match owner {
        Owner::Script => format!("{name}.R"),
        Owner::Package => format!("R/{name}.R"),
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
