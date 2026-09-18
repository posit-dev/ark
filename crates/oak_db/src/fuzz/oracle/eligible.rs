//! Admits the restricted scenario family whose incremental and fresh
//! resolution must agree.
//!
//! Admission reads only scenario inputs. Query results and recovery handlers
//! must not determine eligibility because the oracle detects stale answers and
//! memoized recovery results.
//!
//! Within this domain, every handler in [`crate::recovery`] lacks the
//! dependency edge it would need to re-enter its own key:
//!
//! - `semantic_index()`, `exports()`, and `inherited_layers()` can re-enter only
//!   through a `source()` cycle. [`acyclic()`] rejects cycles in the union of
//!   every program state.
//! - `attached_packages()` and `cross_file_layers()` re-enter through a
//!   collation predecessor. A flat script file gets `load_context()`'s
//!   standalone context, whose `visible_files` is empty, so that edge is never
//!   built.
//! - `attached_packages_anywhere()` has no tracked reader, so nothing can make
//!   it Salsa's repeated key.
//! - `Package::resolve()` remains reachable through `base` on the default
//!   search path. It can recurse only through a `NAMESPACE` re-export, but the
//!   installed stub has none because `World::materialize()` gives it no files
//!   and `EmptyFileReader` returns an empty `NAMESPACE`.
//!
//! The campaign still asserts that no recovery handler fires, so these
//! constraints are checked rather than assumed.

use std::collections::HashMap;
use std::collections::HashSet;

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Block;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Stmt;

use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::Owner;
use crate::fuzz::traversal::sited_programs;
use crate::fuzz::traversal::ProgramSite;

/// Basenames that trigger implicit package attachment without a call in the
/// file. Admitted paths are flat, so layout-specific loaders are excluded.
const LOADER_NAMES: [&str; 5] = [
    "app.R",
    "ui.R",
    "server.R",
    "global.R",
    "_disable_autoload.R",
];

/// Path syntax that cannot be represented by this domain's basename-to-file
/// dependency map.
const PATH_SYNTAX: [char; 3] = ['/', '\\', ':'];

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reject {
    ModeledPackage,
    PackageOwnedFile(usize),
    /// `source()` resolves targets by path, so this domain needs plain basenames
    /// to construct the same dependency edges.
    NonFlatPath(String),
    /// The dependency graph requires one node per file, so paths must be unique.
    DuplicatePath(String),
    LoaderPath(String),
    /// Dependencies from this statement form are outside the admitted graph.
    UnsupportedStatement(String),
    /// Attachments reach `Package::resolve()`, which is outside this domain even
    /// though installed stubs cannot cycle.
    Attach(String),
    /// A directory provider creates edges from a path-based file walk rather than
    /// a single named target.
    DirectoryProvider(String),
    UnmappedSourceTarget(String),
    SourceCycle,
}

/// Checks initial and replacement programs together because edges from any
/// history state can participate in the rejected union cycle.
pub(super) fn admit(scenario: &Scenario) -> Result<(), Reject> {
    if !scenario.initial.packages.is_empty() {
        return Err(Reject::ModeledPackage);
    }

    let mut targets: HashMap<&str, usize> = HashMap::new();
    for (index, file) in scenario.initial.files.iter().enumerate() {
        if file.owner != Owner::Script {
            return Err(Reject::PackageOwnedFile(index));
        }
        if !flat_name(&file.path) {
            return Err(Reject::NonFlatPath(file.path.clone()));
        }
        if LOADER_NAMES.contains(&file.path.as_str()) {
            return Err(Reject::LoaderPath(file.path.clone()));
        }
        if targets.insert(file.path.as_str(), index).is_some() {
            return Err(Reject::DuplicatePath(file.path.clone()));
        }
    }

    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (site, program) in sited_programs(scenario) {
        let Some(sourcing) = sourcing_file(scenario, &site) else {
            continue;
        };
        admit_block(&program.statements, sourcing, &targets, &mut edges)?;
    }

    acyclic(scenario.initial.files.len(), &edges)
}

/// Initial programs belong to their file index. Replacement programs belong to
/// the file targeted by their `Edit`.
fn sourcing_file(scenario: &Scenario, site: &ProgramSite) -> Option<usize> {
    match site {
        ProgramSite::File(index) => Some(*index),
        ProgramSite::Op(index) => match scenario.ops.get(*index) {
            Some(Op::Edit(edit)) => Some(edit.file.0),
            _ => None,
        },
    }
}

/// Conservatively treats `source()` calls in function bodies and runtime-shadowed
/// calls as edges so [`acyclic()`] cannot overlook a possible dependency cycle.
fn admit_block(
    block: &Block,
    sourcing: usize,
    targets: &HashMap<&str, usize>,
    edges: &mut Vec<(usize, usize)>,
) -> Result<(), Reject> {
    for statement in block {
        match statement {
            Stmt::Bind { value, .. } => admit_expr(value, sourcing, targets, edges)?,
            Stmt::Expr(expr) => admit_expr(expr, sourcing, targets, edges)?,
            Stmt::Effect { recipe, .. } => admit_recipe(recipe, sourcing, targets, edges)?,
        }
    }
    Ok(())
}

fn admit_expr(
    expr: &Expr,
    sourcing: usize,
    targets: &HashMap<&str, usize>,
    edges: &mut Vec<(usize, usize)>,
) -> Result<(), Reject> {
    match expr {
        Expr::Num(_) | Expr::Null | Expr::Ident(_) | Expr::Call { .. } => Ok(()),
        Expr::Function { body } => admit_block(body, sourcing, targets, edges),
        Expr::Hole(_) => Err(Reject::UnsupportedStatement("evaluation hole".to_string())),
    }
}

fn admit_recipe(
    recipe: &EffectRecipe,
    sourcing: usize,
    targets: &HashMap<&str, usize>,
    edges: &mut Vec<(usize, usize)>,
) -> Result<(), Reject> {
    match recipe {
        EffectRecipe::Source { path, provider } => {
            if *provider != SourceProvider::File {
                return Err(Reject::DirectoryProvider(path.clone()));
            }
            match targets.get(path.as_str()) {
                Some(target) => {
                    edges.push((sourcing, *target));
                    Ok(())
                },
                None => Err(Reject::UnmappedSourceTarget(path.clone())),
            }
        },
        EffectRecipe::Attach { package } => Err(Reject::Attach(package.clone())),
        EffectRecipe::Assign { .. } => {
            Err(Reject::UnsupportedStatement("assign effect".to_string()))
        },
        EffectRecipe::Rebind { .. } => {
            Err(Reject::UnsupportedStatement("rebind effect".to_string()))
        },
        EffectRecipe::Eval { .. } => Err(Reject::UnsupportedStatement("eval effect".to_string())),
        EffectRecipe::Quote { .. } |
        EffectRecipe::QuoteHoles { .. } |
        EffectRecipe::Substitute { .. } => {
            Err(Reject::UnsupportedStatement("quoted body".to_string()))
        },
    }
}

/// Leading `.` and `~` identify hidden loader files or home-relative paths,
/// not ordinary basenames.
fn flat_name(path: &str) -> bool {
    !path.is_empty() &&
        !path.starts_with('.') &&
        !path.starts_with('~') &&
        !path.contains(PATH_SYNTAX)
}

/// Rejects cycles in the union of all potential edges, conservatively excluding
/// some histories whose individual states are acyclic.
fn acyclic(files: usize, edges: &[(usize, usize)]) -> Result<(), Reject> {
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();

    for file in 0..files {
        if reaches_itself(file, edges, &mut visiting, &mut done) {
            return Err(Reject::SourceCycle);
        }
    }
    Ok(())
}

fn reaches_itself(
    file: usize,
    edges: &[(usize, usize)],
    visiting: &mut HashSet<usize>,
    done: &mut HashSet<usize>,
) -> bool {
    if done.contains(&file) {
        return false;
    }
    if !visiting.insert(file) {
        return true;
    }
    let cyclic = edges
        .iter()
        .filter(|(sourcing, _)| *sourcing == file)
        .any(|(_, target)| reaches_itself(*target, edges, visiting, done));
    visiting.remove(&file);
    done.insert(file);
    cyclic
}

#[cfg(test)]
mod tests {
    use oak_semantic::fuzz::Invocation;
    use oak_semantic::fuzz::Program;

    use super::*;
    use crate::fuzz::build::binding;
    use crate::fuzz::build::library;
    use crate::fuzz::build::quoted;
    use crate::fuzz::build::source;
    use crate::fuzz::build::source_with;
    use crate::fuzz::scenario::Edit;
    use crate::fuzz::scenario::Query;
    use crate::fuzz::spec::FileId;
    use crate::fuzz::spec::FileSpec;
    use crate::fuzz::spec::PackageKind;
    use crate::fuzz::spec::PackageSpec;
    use crate::fuzz::spec::WorkspaceSpec;

    fn script(path: &str, statements: Vec<Stmt>) -> FileSpec {
        FileSpec {
            owner: Owner::Script,
            path: path.to_string(),
            program: Program { statements },
        }
    }

    fn workspace(files: Vec<FileSpec>) -> WorkspaceSpec {
        WorkspaceSpec {
            installed: vec!["base".to_string()],
            packages: Vec::new(),
            files,
        }
    }

    fn scenario(initial: WorkspaceSpec, ops: Vec<Op>) -> Scenario {
        let cold_entry = Query::Resolve(FileId(0), "val_0".to_string());
        Scenario::cold(initial, cold_entry, ops)
    }

    fn edit(file: usize, statements: Vec<Stmt>) -> Op {
        Op::Edit(Edit {
            file: FileId(file),
            program: Program { statements },
        })
    }

    #[test]
    fn test_admits_a_flat_script_chain() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("b.R", vec![source("a.R"), binding("val_1")]),
        ]);

        assert_eq!(admit(&scenario(initial, vec![])), Ok(()));
    }

    #[test]
    fn test_admits_a_source_call_in_a_function_body() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("b.R", vec![crate::fuzz::build::function_def("read", vec![
                source("a.R"),
            ])]),
        ]);

        assert_eq!(admit(&scenario(initial, vec![])), Ok(()));
    }

    #[test]
    fn test_rejects_a_modeled_package() {
        let mut initial = workspace(vec![script("a.R", vec![binding("val_0")])]);
        initial.packages.push(PackageSpec {
            name: "mypkg".to_string(),
            kind: PackageKind::Workspace,
            exports: Vec::new(),
            reexports: Vec::new(),
        });

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::ModeledPackage)
        );
    }

    #[test]
    fn test_rejects_a_package_owned_file() {
        let mut initial = workspace(vec![script("a.R", vec![binding("val_0")])]);
        initial.files[0].owner = Owner::Package(crate::fuzz::spec::PackageId(0));

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::PackageOwnedFile(0))
        );
    }

    #[test]
    fn test_rejects_an_attach() {
        let initial = workspace(vec![script("a.R", vec![library("magrittr")])]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::Attach("magrittr".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_directory_provider() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("b.R", vec![source_with(
                "a.R",
                SourceProvider::Dir,
                Invocation::Bare,
            )]),
        ]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::DirectoryProvider("a.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_an_unmapped_source_target() {
        let initial = workspace(vec![script("a.R", vec![source("gone.R")])]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::UnmappedSourceTarget("gone.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_quoted_body() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("b.R", vec![quoted(vec![source("a.R")])]),
        ]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::UnsupportedStatement("quoted body".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_loader_path() {
        let initial = workspace(vec![script("app.R", vec![binding("val_0")])]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::LoaderPath("app.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_path_under_an_r_directory() {
        let initial = workspace(vec![script("R/a.R", vec![binding("val_0")])]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::NonFlatPath("R/a.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_nested_path() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("sub/d.R", vec![binding("val_1")]),
        ]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::NonFlatPath("sub/d.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_parent_directory_component() {
        let initial = workspace(vec![script("../a.R", vec![binding("val_0")])]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::NonFlatPath("../a.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_hidden_loader_file() {
        let initial = workspace(vec![script(".Rprofile", vec![binding("val_0")])]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::NonFlatPath(".Rprofile".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_duplicate_path() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("a.R", vec![binding("val_1")]),
        ]);

        assert_eq!(
            admit(&scenario(initial, vec![])),
            Err(Reject::DuplicatePath("a.R".to_string()))
        );
    }

    #[test]
    fn test_rejects_a_cycle_in_the_initial_workspace() {
        let initial = workspace(vec![
            script("a.R", vec![source("b.R")]),
            script("b.R", vec![source("a.R")]),
        ]);

        assert_eq!(admit(&scenario(initial, vec![])), Err(Reject::SourceCycle));
    }

    #[test]
    fn test_rejects_a_cycle_introduced_by_a_replacement_program() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("b.R", vec![source("a.R")]),
        ]);
        let ops = vec![edit(0, vec![source("b.R")])];

        assert_eq!(admit(&scenario(initial, ops)), Err(Reject::SourceCycle));
    }

    /// Admission remains conservative because the edge union is cyclic even
    /// though the final edit removes the live cycle.
    #[test]
    fn test_rejects_a_cycle_that_a_later_edit_reopens() {
        let initial = workspace(vec![
            script("a.R", vec![binding("val_0")]),
            script("b.R", vec![source("a.R")]),
        ]);
        let ops = vec![
            edit(0, vec![source("b.R")]),
            edit(0, vec![binding("val_0")]),
        ];

        assert_eq!(admit(&scenario(initial, ops)), Err(Reject::SourceCycle));
    }
}
