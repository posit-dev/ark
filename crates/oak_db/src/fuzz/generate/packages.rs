//! Builds the package re-export layers used by source-graph drafts.

use rand::rngs::StdRng;
use rand::RngExt;

use super::FileParts;
use crate::fuzz::choose::export_name;
use crate::fuzz::scenario::Query;
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
    };
    let mut lib1 = PackageSpec {
        name: REEXPORT_LIBS[1].to_string(),
        kind: PackageKind::Library,
        exports: vec![name.clone()],
        reexports: Vec::new(),
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
            let index = rng.random_range(0..parts.len());
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
    use super::*;
    use crate::fuzz::generate::MOTIFS;

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
