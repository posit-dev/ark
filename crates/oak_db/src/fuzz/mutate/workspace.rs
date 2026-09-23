//! Mutates files and packages, repairing identifiers after removals.

use oak_semantic::fuzz::Program;

use super::pick;
use crate::fuzz::budgets::MAX_EXPORTS;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::budgets::MAX_PACKAGES;
use crate::fuzz::budgets::MAX_REEXPORTS;
use crate::fuzz::build::binding;
use crate::fuzz::choose::binding_name;
use crate::fuzz::choose::export_name;
use crate::fuzz::choose::name_vocabulary;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Choose;
use crate::fuzz::choose::Shape;
use crate::fuzz::choose::EXPORT_NAMES;
use crate::fuzz::generate::file_path;
use crate::fuzz::generate::UNINSTALLED;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::Reexport;

// == Choice vocabularies ==

const PACKAGE_NAMES: [&str; 3] = ["lib0", "lib1", "lib2"];

// == Files ==

pub(super) fn add_file(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.initial.files.len() >= MAX_FILES {
        return;
    }
    let mut owners = vec![Owner::Script];
    owners.extend(
        scenario
            .initial
            .packages
            .iter()
            .enumerate()
            .filter_map(|(index, package)| {
                (package.kind == PackageKind::Workspace).then_some(Owner::Package(PackageId(index)))
            }),
    );
    let owner = owners[rng.index(owners.len())];
    let Some(index) = (0..MAX_FILES).find(|&index| {
        let path = file_path(owner, index);
        // Different packages can each own `R/a.R` under their own roots.
        !scenario
            .initial
            .files
            .iter()
            .any(|file| file.owner == owner && file.path == path)
    }) else {
        return;
    };

    scenario.initial.files.push(FileSpec {
        owner,
        path: file_path(owner, index),
        program: Program {
            statements: vec![binding(&binding_name(index))],
        },
    });
}

pub(super) fn remove_file(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.initial.files.len() <= 1 {
        return;
    }
    let removed = rng.index(scenario.initial.files.len());
    scenario.initial.files.remove(removed);
    repair_file_ids(scenario, removed);
}

/// Retarget operations to live files. Leave stale `source()` paths intact so
/// the runner still exercises unresolved files.
fn repair_file_ids(scenario: &mut Scenario, removed: usize) {
    let count = scenario.initial.files.len();
    if count == 0 {
        return;
    }
    if let Some(file) = scenario.cold_entry.file_mut() {
        rebase_file(file, removed, count);
    }
    for op in &mut scenario.ops {
        if let Some(file) = op.file_mut() {
            rebase_file(file, removed, count);
        }
    }
}

fn rebase_file(file: &mut FileId, removed: usize, count: usize) {
    let index = match file.0 {
        index if index < removed => index,
        index if index > removed => index - 1,
        _ => removed,
    };
    file.0 = index.min(count - 1);
}

// == Packages ==

fn export_vocabulary() -> Vec<String> {
    name_vocabulary()
}

pub(super) fn add_export(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_export_room(scenario)) else {
        return;
    };
    let exports = &mut scenario.initial.packages[index].exports;
    let free = export_vocabulary()
        .into_iter()
        .filter(|name| !exports.contains(name))
        .collect();
    let Some(export) = pick(rng, free) else {
        panic!("export candidate has no available names");
    };
    exports.push(export);
}

pub(super) fn remove_export(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_exports(scenario)) else {
        return;
    };
    let exports = &mut scenario.initial.packages[index].exports;
    let removed = rng.index(exports.len());
    exports.remove(removed);
}

pub(super) fn packages_with_export_room(scenario: &Scenario) -> Vec<usize> {
    (0..scenario.initial.packages.len())
        .filter(|&index| {
            let exports = &scenario.initial.packages[index].exports;
            exports.len() < MAX_EXPORTS &&
                export_vocabulary()
                    .iter()
                    .any(|name| !exports.contains(name))
        })
        .collect()
}

pub(super) fn packages_with_exports(scenario: &Scenario) -> Vec<usize> {
    (0..scenario.initial.packages.len())
        .filter(|&index| !scenario.initial.packages[index].exports.is_empty())
        .collect()
}

pub(super) fn add_reexport_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_reexport_room(scenario)) else {
        return;
    };
    let from = reexport_source(rng, scenario);
    // Keep one `importFrom()` per name so parsing cannot discard an edge.
    let taken: Vec<&str> = scenario.initial.packages[index]
        .reexports
        .iter()
        .map(|reexport| reexport.name.as_str())
        .collect();
    let free: Vec<String> = (0..EXPORT_NAMES)
        .map(export_name)
        .filter(|name| !taken.contains(&name.as_str()))
        .collect();
    let Some(name) = pick(rng, free) else {
        return;
    };
    scenario.initial.packages[index]
        .reexports
        .push(Reexport { name, from });
}

pub(super) fn redirect_reexport_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some((package, reexport)) = pick(rng, reexport_slots(scenario)) else {
        return;
    };
    let current = scenario.initial.packages[package].reexports[reexport]
        .from
        .clone();
    let others: Vec<String> = reexport_sources(scenario)
        .into_iter()
        .filter(|candidate| *candidate != current)
        .collect();
    let Some(from) = pick(rng, others) else {
        return;
    };
    scenario.initial.packages[package].reexports[reexport].from = from;
}

pub(super) fn remove_reexport_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some((package, reexport)) = pick(rng, reexport_slots(scenario)) else {
        return;
    };
    scenario.initial.packages[package]
        .reexports
        .remove(reexport);
}

pub(super) fn packages_with_reexport_room(scenario: &Scenario) -> Vec<usize> {
    (0..scenario.initial.packages.len())
        .filter(|&index| {
            let reexports = &scenario.initial.packages[index].reexports;
            reexports.len() < MAX_REEXPORTS &&
                (0..EXPORT_NAMES)
                    .map(export_name)
                    .any(|name| !reexports.iter().any(|edge| edge.name == name))
        })
        .collect()
}

pub(super) fn reexport_slots(scenario: &Scenario) -> Vec<(usize, usize)> {
    scenario
        .initial
        .packages
        .iter()
        .enumerate()
        .flat_map(|(package, spec)| {
            (0..spec.reexports.len()).map(move |reexport| (package, reexport))
        })
        .collect()
}

fn reexport_source(rng: &mut impl Choose, scenario: &Scenario) -> String {
    let sources = reexport_sources(scenario);
    sources[rng.index(sources.len())].clone()
}

/// Modeled package names plus one known-absent name, so a dangling import
/// stays reachable.
fn reexport_sources(scenario: &Scenario) -> Vec<String> {
    let mut sources: Vec<String> = scenario
        .initial
        .packages
        .iter()
        .map(|package| package.name.clone())
        .collect();
    sources.push(UNINSTALLED.to_string());
    sources
}

pub(super) fn add_package(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.initial.packages.len() >= MAX_PACKAGES {
        return;
    }
    let Some(name) = pick(rng, available_package_names(scenario)) else {
        return;
    };
    scenario.initial.packages.push(PackageSpec {
        name: name.to_string(),
        kind: if rng.odds(50) {
            PackageKind::Library
        } else {
            PackageKind::Workspace
        },
        exports: Vec::new(),
        reexports: Vec::new(),
        collate: None,
    });
}

pub(super) fn available_package_names(scenario: &Scenario) -> Vec<&'static str> {
    PACKAGE_NAMES
        .into_iter()
        .filter(|name| {
            !scenario
                .initial
                .packages
                .iter()
                .any(|package| package.name == *name) &&
                !scenario
                    .initial
                    .installed
                    .iter()
                    .any(|installed| installed == name)
        })
        .collect()
}

/// Delete owned files with the package. Moving them to another package would
/// change the root used to resolve their `source()` paths.
pub(super) fn remove_package(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(removed) = pick(rng, removable_packages(scenario)) else {
        return;
    };
    remove_owned_files(scenario, removed);
    scenario.initial.packages.remove(removed);
    repair_package_ids(rng, scenario, removed);
}

/// Packages whose removal still leaves at least one file, matching the
/// `validate()` rule that a workspace has files.
pub(super) fn removable_packages(scenario: &Scenario) -> Vec<usize> {
    let total = scenario.initial.files.len();
    (0..scenario.initial.packages.len())
        .filter(|&index| owned_file_count(scenario, index) < total)
        .collect()
}

fn owned_file_count(scenario: &Scenario, package: usize) -> usize {
    scenario
        .initial
        .files
        .iter()
        .filter(|file| matches!(file.owner, Owner::Package(id) if id.0 == package))
        .count()
}

/// Remove higher file indices first so pending removals keep their indices.
fn remove_owned_files(scenario: &mut Scenario, package: usize) {
    let owned: Vec<usize> = scenario
        .initial
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| matches!(file.owner, Owner::Package(id) if id.0 == package))
        .map(|(index, _)| index)
        .collect();
    for index in owned.into_iter().rev() {
        scenario.initial.files.remove(index);
        repair_file_ids(scenario, index);
    }
}

/// Retarget queries to surviving packages. If none remain, replace package
/// queries with queries supported by the remaining workspace.
fn repair_package_ids(rng: &mut impl Choose, scenario: &mut Scenario, removed: usize) {
    let count = scenario.initial.packages.len();
    if count == 0 {
        let shape = Shape::of(&scenario.initial);
        if scenario.cold_entry.package().is_some() {
            scenario.cold_entry = random_query(rng, &shape);
        }
        for op in &mut scenario.ops {
            if let Op::Query(query) = op {
                if query.package().is_some() {
                    *query = random_query(rng, &shape);
                }
            }
        }
        return;
    }

    if let Some(id) = scenario.cold_entry.package_mut() {
        rebase_package(id, removed, count);
    }
    for op in &mut scenario.ops {
        if let Op::Query(query) = op {
            if let Some(id) = query.package_mut() {
                rebase_package(id, removed, count);
            }
        }
    }
    for file in &mut scenario.initial.files {
        if let Owner::Package(id) = &mut file.owner {
            rebase_package(id, removed, count);
        }
    }
}

fn rebase_package(id: &mut PackageId, removed: usize, count: usize) {
    let index = match id.0 {
        index if index < removed => index,
        index if index > removed => index - 1,
        _ => removed,
    };
    id.0 = index.min(count - 1);
}
