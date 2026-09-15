//! Random choices shared by the seed generator and the mutators.

use rand::rngs::StdRng;
use rand::RngExt;

use crate::file_imports::CollationView;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Site;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::WorkspaceSpec;
use crate::NamespaceVisibility;

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

pub(super) const EXPORT_NAMES: usize = 3;

pub(super) fn export_name(index: usize) -> String {
    format!("exp_{index}")
}

/// Includes declared export names so queries can pass the export gate and
/// follow re-export chains.
pub(super) struct Shape {
    pub(super) files: usize,
    pub(super) packages: usize,
    exports: Vec<String>,
}

impl Shape {
    pub(super) fn of(spec: &WorkspaceSpec) -> Shape {
        let mut exports: Vec<String> = spec
            .packages
            .iter()
            .flat_map(|package| package.exports.iter().cloned())
            .collect();
        exports.sort();
        exports.dedup();

        Shape {
            files: spec.files.len(),
            packages: spec.packages.len(),
            exports,
        }
    }

    /// Usually selects a declared export. Fixed-vocabulary draws also exercise
    /// rejection at the export gate.
    fn export(&self, rng: &mut impl Choose) -> String {
        if self.exports.is_empty() || rng.odds(20) {
            return export_name(rng.index(EXPORT_NAMES));
        }
        self.exports[rng.index(self.exports.len())].clone()
    }
}

/// Give every entry query a fresh database because Salsa's repeated key depends
/// on entry order.
pub(super) fn cold_entries(rng: &mut impl Choose, shape: &Shape) -> Vec<Query> {
    vec![
        cycle_entry(rng, shape),
        aggregate_entry(rng),
        production_entry(rng, shape),
        random_query(rng, shape),
    ]
}

pub(super) fn random_query(rng: &mut impl Choose, shape: &Shape) -> Query {
    match rng.index(3) {
        0 => production_entry(rng, shape),
        1 => aggregate_entry(rng),
        _ => cycle_entry(rng, shape),
    }
}

/// Enter each file-keyed `cycle_result` query directly, plus a direct
/// `Package::resolve()` entry when the workspace models any package.
fn cycle_entry(rng: &mut impl Choose, shape: &Shape) -> Query {
    let file = random_file(rng, shape.files);
    let view = random_view(rng);
    let options = if shape.packages > 0 { 7 } else { 6 };
    match rng.index(options) {
        0 => Query::SemanticIndex(file),
        1 => Query::Exports(file),
        2 => Query::AttachedPackages(file),
        3 => Query::AttachedPackagesAnywhere(file),
        4 => Query::InheritedLayers(file, view),
        5 => Query::CrossFileLayers(file, view),
        _ => random_package_resolve(rng, shape),
    }
}

fn random_package_resolve(rng: &mut impl Choose, shape: &Shape) -> Query {
    let id = PackageId(rng.index(shape.packages));
    let name = shape.export(rng);
    let visibility = if rng.odds(50) {
        NamespaceVisibility::Exported
    } else {
        NamespaceVisibility::Internal
    };
    Query::PackageResolve(id, name, visibility)
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

fn production_entry(rng: &mut impl Choose, shape: &Shape) -> Query {
    let file = random_file(rng, shape.files);
    match rng.index(7) {
        0 => Query::Diagnostics(file),
        1 => Query::Imports(file),
        2 => Query::ImportsAt(file, random_site(rng)),
        3 => Query::ResolveAt(file, random_site(rng)),
        4 => Query::Resolve(file, random_name(rng, shape)),
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

/// Include `source` to query the shadowed-call case, and an export name so a
/// file that attaches a modeled package can resolve through its re-exports.
fn random_name(rng: &mut impl Choose, shape: &Shape) -> String {
    if rng.odds(20) {
        return "source".to_string();
    }
    if rng.odds(30) {
        return shape.export(rng);
    }
    binding_name(rng.index(shape.files))
}
