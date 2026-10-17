//! Builds the package re-export layers used by source-graph drafts.

use rand::rngs::StdRng;
use rand::RngExt;

use super::FileParts;
use crate::fuzz::choose::export_name;
use crate::fuzz::scenario::Query;
use crate::fuzz::spec::is_collation_member;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::Reexport;
use crate::NamespaceVisibility;

/// Name of the draft's single workspace package, when `owner` is
/// `Owner::Package`.
pub(super) const WORKSPACE_PACKAGE: &str = "mypkg";

/// Names of the two `Library` packages [`reexport_layer()`] chains together.
const REEXPORT_LIBS: [&str; 2] = ["lib0", "lib1"];

/// Whether a draft models re-export packages, and how its chain ends. Assigned
/// by motif position with a seed-dependent rotation, so every seed corpus
/// contains each layer without fixing its source-graph pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PackageLayer {
    /// No re-export layer. Any workspace package has an empty namespace.
    Bare,
    /// `lib0` re-exports the name from `lib1`, which exports but never binds
    /// it. Acyclic, and resolves to nothing.
    Chain,
    /// `lib0` and `lib1` re-export the name from each other, so
    /// `Package::resolve()` revisits its own key.
    Cycle,
    /// The chain ends at the workspace package, which binds and exports the
    /// name in one of its files.
    Local,
}

const PACKAGE_LAYERS: [PackageLayer; 4] = [
    PackageLayer::Bare,
    PackageLayer::Chain,
    PackageLayer::Cycle,
    PackageLayer::Local,
];

pub(super) fn package_layer(seed: u64, motif: usize) -> PackageLayer {
    let offset = (seed % PACKAGE_LAYERS.len() as u64) as usize;
    PACKAGE_LAYERS[(motif % PACKAGE_LAYERS.len() + offset) % PACKAGE_LAYERS.len()]
}

/// The chain always starts at `lib0`, which sits after the workspace package
/// when the draft has one.
pub(super) fn package_entry(layer: PackageLayer, owner: Owner) -> Option<Query> {
    if layer == PackageLayer::Bare {
        return None;
    }
    let lib0 = match owner {
        Owner::Package(_) => 1,
        Owner::Script => 0,
    };
    Some(Query::PackageResolve(
        PackageId(lib0),
        export_name(0),
        NamespaceVisibility::Exported,
    ))
}

pub(super) fn empty_package(name: &str) -> PackageSpec {
    PackageSpec {
        name: name.to_string(),
        kind: PackageKind::Workspace,
        exports: Vec::new(),
        reexports: Vec::new(),
        collate: None,
    }
}

/// Supplies re-export edges for mutation. A locally terminating chain also
/// needs a workspace package and a definition in one of its files.
pub(super) fn reexport_layer(
    rng: &mut StdRng,
    layer: PackageLayer,
    parts: &mut [FileParts],
) -> (Option<PackageSpec>, Vec<PackageSpec>) {
    if layer == PackageLayer::Bare {
        return (None, Vec::new());
    }

    let name = export_name(0);
    let lib0 = PackageSpec {
        name: REEXPORT_LIBS[0].to_string(),
        kind: PackageKind::Library,
        exports: vec![name.clone()],
        reexports: vec![Reexport {
            name: name.clone(),
            from: REEXPORT_LIBS[1].to_string(),
        }],
        collate: None,
    };
    let mut lib1 = PackageSpec {
        name: REEXPORT_LIBS[1].to_string(),
        kind: PackageKind::Library,
        exports: vec![name.clone()],
        reexports: Vec::new(),
        collate: None,
    };

    let workspace_package = match layer {
        // `lib1` exports the name but re-exports nothing further, so the chain
        // dead-ends without closing a cycle.
        PackageLayer::Bare | PackageLayer::Chain => None,
        PackageLayer::Cycle => {
            lib1.reexports.push(Reexport {
                name: name.clone(),
                from: REEXPORT_LIBS[0].to_string(),
            });
            None
        },
        PackageLayer::Local => {
            lib1.reexports.push(Reexport {
                name: name.clone(),
                from: WORKSPACE_PACKAGE.to_string(),
            });
            // `Package::resolve()` searches `package.files()`, so the local
            // export must be defined in the collation rather than a script.
            let members: Vec<usize> = parts
                .iter()
                .enumerate()
                .filter(|(_, part)| is_collation_member(&part.path))
                .map(|(index, _)| index)
                .collect();
            let index = match members.as_slice() {
                [] => panic!(
                    "harness bug: a `Local` draft has no collation member to define its export"
                ),
                members => members[rng.random_range(0..members.len())],
            };
            parts[index].local_export = Some(name.clone());
            let mut package = empty_package(WORKSPACE_PACKAGE);
            package.exports.push(name);
            Some(package)
        },
    };

    (workspace_package, vec![lib0, lib1])
}

#[cfg(test)]
mod tests {
    use oak_semantic::fuzz::Stmt;

    use super::*;
    use crate::fuzz::generate::seed_corpus;
    use crate::fuzz::generate::MOTIFS;
    use crate::fuzz::spec::is_collation_member;

    /// Ensures each `Local` re-export terminates in `package.files()`, where
    /// `Package::resolve()` searches. Source cycles can prevent resolution even
    /// when the export is correctly placed, so this inspects its defining file.
    #[test]
    fn test_local_export_is_defined_in_the_collation() {
        for seed in 0..6u64 {
            for scenario in seed_corpus(seed) {
                let workspace = scenario
                    .initial
                    .packages
                    .iter()
                    .find(|package| package.kind == PackageKind::Workspace);
                let Some(workspace) = workspace else {
                    continue;
                };

                for export in &workspace.exports {
                    let defining: Vec<&str> = scenario
                        .initial
                        .files
                        .iter()
                        .filter(|file| defines(&file.program.statements, export))
                        .map(|file| file.path.as_str())
                        .collect();

                    assert!(!defining.is_empty());
                    for path in defining {
                        assert!(is_collation_member(path));
                    }
                }
            }
        }
    }

    fn defines(statements: &[Stmt], name: &str) -> bool {
        statements
            .iter()
            .any(|stmt| matches!(stmt, Stmt::Bind { name: bound, .. } if bound == name))
    }

    #[test]
    fn test_package_layer_rotation() {
        for motif in 0..MOTIFS.len() {
            let actual: Vec<_> = (0..4).map(|seed| package_layer(seed, motif)).collect();
            let expected: Vec<_> = (0..4)
                .map(|offset| PACKAGE_LAYERS[(motif + offset) % 4])
                .collect();
            assert_eq!(actual, expected);
        }

        for seed in [0, 1, 2, 3, 4, u64::MAX] {
            let actual: Vec<_> = (0..MOTIFS.len())
                .map(|motif| package_layer(seed, motif))
                .collect();
            for layer in PACKAGE_LAYERS {
                assert!(actual.contains(&layer));
            }
            assert_eq!(package_layer(seed, 0), PACKAGE_LAYERS[(seed % 4) as usize]);
        }
    }
}
