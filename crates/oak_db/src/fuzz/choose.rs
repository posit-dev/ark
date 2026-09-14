//! Random choices shared by the seed generator and the mutators.

use rand::rngs::StdRng;
use rand::RngExt;

use crate::file_imports::CollationView;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Site;
use crate::fuzz::spec::FileId;

/// Unifies [`StdRng`] and [`mutatis::Rng`] for shared query generation.
pub(super) trait Choose {
    /// `len` must be positive.
    fn index(&mut self, len: usize) -> usize;

    fn odds(&mut self, percent: u32) -> bool;
}

impl Choose for StdRng {
    fn index(&mut self, len: usize) -> usize {
        self.random_range(0..len)
    }

    fn odds(&mut self, percent: u32) -> bool {
        self.random_range(0..100u32) < percent
    }
}

impl Choose for mutatis::Rng {
    fn index(&mut self, len: usize) -> usize {
        self.gen_index(len).unwrap_or(0)
    }

    fn odds(&mut self, percent: u32) -> bool {
        self.index(100) < percent as usize
    }
}

pub(super) fn binding_name(index: usize) -> String {
    format!("val_{index}")
}

/// Give every entry query a fresh database because Salsa's repeated key depends
/// on entry order.
pub(super) fn cold_entries(rng: &mut impl Choose, files: usize) -> Vec<Query> {
    vec![
        cycle_entry(rng, files),
        aggregate_entry(rng),
        production_entry(rng, files),
        random_query(rng, files),
    ]
}

pub(super) fn random_query(rng: &mut impl Choose, files: usize) -> Query {
    match rng.index(3) {
        0 => production_entry(rng, files),
        1 => aggregate_entry(rng),
        _ => cycle_entry(rng, files),
    }
}

/// Enter each file-keyed `cycle_result` query directly. `Package::resolve()`
/// needs NAMESPACE re-exports, which these workspaces do not provide.
fn cycle_entry(rng: &mut impl Choose, files: usize) -> Query {
    let file = random_file(rng, files);
    let view = random_view(rng);
    match rng.index(6) {
        0 => Query::SemanticIndex(file),
        1 => Query::Exports(file),
        2 => Query::AttachedPackages(file),
        3 => Query::AttachedPackagesAnywhere(file),
        4 => Query::InheritedLayers(file, view),
        _ => Query::CrossFileLayers(file, view),
    }
}

/// Run aggregates first to exercise their cold-entry cycle behavior.
fn aggregate_entry(rng: &mut impl Choose) -> Query {
    match rng.index(5) {
        0 => Query::AllPackageDependencies,
        1 => Query::AllWorkspaceFileDependencies,
        2 => Query::AllWorkspaceLoaderDependencies,
        3 => Query::AllWorkspacePackageDependencies,
        _ => Query::DefaultSearchPathPackages,
    }
}

fn production_entry(rng: &mut impl Choose, files: usize) -> Query {
    let file = random_file(rng, files);
    match rng.index(7) {
        0 => Query::Diagnostics(file),
        1 => Query::Imports(file),
        2 => Query::ImportsAt(file, random_site(rng)),
        3 => Query::ResolveAt(file, random_site(rng)),
        4 => Query::Resolve(file, random_name(rng, files)),
        5 => Query::UsedPackages(file),
        _ => Query::SourcedBy(file),
    }
}

fn random_file(rng: &mut impl Choose, files: usize) -> FileId {
    FileId(rng.index(files))
}

fn random_site(rng: &mut impl Choose) -> Site {
    match rng.index(3) {
        0 => Site::FirstCall,
        1 => Site::LastIdentifier,
        _ => Site::Eof,
    }
}

fn random_view(rng: &mut impl Choose) -> CollationView {
    if rng.odds(50) {
        CollationView::Eager
    } else {
        CollationView::Deferred
    }
}

/// Include `source` to query the shadowed-call case.
fn random_name(rng: &mut impl Choose, files: usize) -> String {
    if rng.odds(20) {
        return "source".to_string();
    }
    binding_name(rng.index(files))
}
