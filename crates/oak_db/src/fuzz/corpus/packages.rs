//! Named package and re-export scenarios.

use super::file_specs;
use super::package_spec;
use super::program;
use super::scenario;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::source;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::WorkspaceSpec;
use crate::NamespaceVisibility;

/// `pkga` has no local definition, so resolution follows its import to `pkgb`.
pub(super) fn acyclic_reexport_chain_resolves_to_the_definition() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("pkga", PackageKind::Workspace, &["exp_a"], &[(
                "exp_a", "pkgb",
            )]),
            package_spec("pkgb", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: file_specs(Owner::Package(PackageId(1)), vec![(
            "R/a.R",
            program(vec![function_def("exp_a", vec![])]),
        )]),
    };
    scenario(
        initial,
        Query::PackageResolve(
            PackageId(0),
            "exp_a".to_string(),
            NamespaceVisibility::Exported,
        ),
        vec![],
    )
}

/// Neither package defines `exp_a` locally. Following their mutual re-exports
/// re-enters `Package::resolve()` with the same key.
pub(super) fn mutual_reexport_has_no_terminal_definition() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec![],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib1",
            )]),
            package_spec("lib1", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib0",
            )]),
        ],
        // Library packages own no files. A lone script keeps the workspace
        // non-empty so `validate()` accepts the scenario.
        files: file_specs(Owner::Script, vec![(
            "a.R",
            program(vec![binding("val_a")]),
        )]),
    };
    scenario(
        initial,
        Query::PackageResolve(
            PackageId(0),
            "exp_a".to_string(),
            NamespaceVisibility::Exported,
        ),
        vec![],
    )
}

/// `lib0` re-exports from `lib1`, which re-exports from the workspace package
/// `pkgw`, which defines `exp_a` locally. Separates chain depth (two hops
/// through metadata-only packages) from the mutual-cycle case.
pub(super) fn reexport_chain_terminates_at_a_local_export() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec![],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib1",
            )]),
            package_spec("lib1", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "pkgw",
            )]),
            package_spec("pkgw", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: file_specs(Owner::Package(PackageId(2)), vec![(
            "R/a.R",
            program(vec![function_def("exp_a", vec![])]),
        )]),
    };
    scenario(
        initial,
        Query::PackageResolve(
            PackageId(0),
            "exp_a".to_string(),
            NamespaceVisibility::Exported,
        ),
        vec![],
    )
}

/// A script attaches `lib0`, which re-exports `exp_a` from the workspace
/// package `pkgw`. `File::resolve()` reaches `Package::resolve()` through the
/// attach's `ImportLayer::Package`, not through a direct `PackageResolve` entry.
pub(super) fn attached_package_consumer_resolves_a_reexport() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "pkgw",
            )]),
            package_spec("pkgw", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: {
            let mut files = file_specs(Owner::Package(PackageId(1)), vec![(
                "R/a.R",
                program(vec![function_def("exp_a", vec![])]),
            )]);
            files.extend(file_specs(Owner::Script, vec![(
                "a.R",
                program(vec![library("lib0")]),
            )]));
            files
        },
    };
    scenario(
        initial,
        Query::Resolve(FileId(1), "exp_a".to_string()),
        vec![],
    )
}

/// The attaching script enters the mutual re-export cycle through file
/// resolution, exercising recovery from a consumer request.
pub(super) fn attached_package_consumer_degrades_on_a_reexport_cycle() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib1",
            )]),
            package_spec("lib1", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib0",
            )]),
        ],
        files: file_specs(Owner::Script, vec![("a.R", program(vec![library("lib0")]))]),
    };
    scenario(
        initial,
        Query::Resolve(FileId(0), "exp_a".to_string()),
        vec![],
    )
}

/// `pkgc`'s own NAMESPACE carries `importFrom(pkgd, exp_a)` with no matching
/// `export()`, so a file inside `pkgc` sees `exp_a` through
/// `ImportLayer::From`, the collation-wide re-export layer, rather than
/// through an attach.
pub(super) fn namespace_import_layer_consumer_resolves_a_reexport() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("pkgc", PackageKind::Workspace, &[], &[("exp_a", "pkgd")]),
            package_spec("pkgd", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: {
            let mut files = file_specs(Owner::Package(PackageId(0)), vec![(
                "R/a.R",
                program(vec![binding("val_c")]),
            )]);
            files.extend(file_specs(Owner::Package(PackageId(1)), vec![(
                "R/a.R",
                program(vec![function_def("exp_a", vec![])]),
            )]));
            files
        },
    };
    scenario(
        initial,
        Query::Resolve(FileId(0), "exp_a".to_string()),
        vec![],
    )
}

/// `lib0` exports `source`, so attaching it binds `source` as a plain export
/// (no registered effect) that shadows `base`'s `source()` effect through
/// `package_binding()`, before the search reaches `base`.
pub(super) fn package_export_shadows_the_source_effect() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![package_spec("lib0", PackageKind::Library, &["source"], &[])],
        files: {
            let mut files = file_specs(Owner::Script, vec![(
                "a.R",
                program(vec![library("lib0"), source("b.R")]),
            )]);
            files.extend(file_specs(Owner::Script, vec![(
                "b.R",
                program(vec![binding("val_b")]),
            )]));
            files
        },
    };
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}
