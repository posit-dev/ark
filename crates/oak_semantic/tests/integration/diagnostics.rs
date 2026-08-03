//! Diagnostics the builder records regardless of which package contributed the
//! effect. Package-specific diagnostic coverage lives under `contrib/`.

use oak_semantic::semantic_index::SemanticDiagnostic;

use crate::common::build_with;
use crate::resolvers::MissingPackageResolver;

#[test]
fn test_attach_records_uninstalled_package() {
    // `MissingPackageResolver` never resolves a package, so the attach
    // records an `UninstalledPackage` diagnostic pointing at the whole
    // `library()` call.
    let source = "library(shiny)\n";
    let index = build_with(source, MissingPackageResolver);

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::UninstalledPackage { package, range } => {
            assert_eq!(package, "shiny");
            let start = u32::from(range.start()) as usize;
            let end = u32::from(range.end()) as usize;
            assert_eq!(&source[start..end], "library(shiny)");
        },
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}
