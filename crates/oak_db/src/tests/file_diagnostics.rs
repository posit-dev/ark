use biome_rowan::TextRange;
use biome_rowan::TextSize;

use crate::tests::resolver::install_packages;
use crate::tests::test_db::file_path;
use crate::tests::test_db::TestDb;
use crate::DiagnosticKind;
use crate::File;
use crate::FileRevision;

fn new_file(db: &mut TestDb, name: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(name),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

/// The range of the `n`th (0-indexed) occurrence of `needle` in `source`.
fn nth_range_of(source: &str, needle: &str, n: usize) -> TextRange {
    let (start, _) = source.match_indices(needle).nth(n).unwrap();
    let start = TextSize::from(start as u32);
    TextRange::new(start, start + TextSize::from(needle.len() as u32))
}

fn range_of(source: &str, needle: &str) -> TextRange {
    nth_range_of(source, needle, 0)
}

#[test]
fn test_diagnostics_lowers_lazy_shadow() {
    // `local`'s NSE reading could be shadowed by the later `local <- identity`
    // at file scope, with undetermined timing relative to `f`'s call. Base's
    // `local()` NSE annotation resolves fixture-free through the real
    // `SalsaImportsResolver` (it falls back to the static base registry).
    let source = "f <- function() local({ x <- 1 })\nlocal <- identity\n";
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", source);

    let diagnostics = file.diagnostics(&db);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];

    assert_eq!(diagnostic.kind(), DiagnosticKind::EffectAmbiguity);
    assert_eq!(
        diagnostic.message(),
        "Ambiguous reading of effectful `local()`. An assignment to `local` in \
         an enclosing scope could run before this call and change its effect."
    );
    assert_eq!(diagnostic.range(), range_of(source, "local({ x <- 1 })"));

    assert_eq!(diagnostic.annotations().len(), 1);
    let annotation = &diagnostic.annotations()[0];
    assert_eq!(annotation.range, nth_range_of(source, "local", 1));
    assert_eq!(annotation.message, "could run before the call");
}

#[test]
fn test_diagnostics_lowers_conditional_shadow() {
    // The inner `local({ y <- 1 })` is flagged because a conditional binding
    // of `local` in the same (outer, eager NSE) scope could shadow it. Base
    // NSE, so no package fixture is needed.
    let source = "\
local({
    if (cond) local <- identity
    local({
        y <- 1
    })
})
";
    let mut db = TestDb::new();
    let file = new_file(&mut db, "a.R", source);

    let diagnostics = file.diagnostics(&db);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];

    assert_eq!(diagnostic.kind(), DiagnosticKind::EffectAmbiguity);
    assert_eq!(
        diagnostic.message(),
        "Ambiguous reading of effectful `local()`. A conditional assignment \
         could shadow `local` on some paths and change its effect."
    );
    assert_eq!(
        diagnostic.range(),
        range_of(source, "local({\n        y <- 1\n    })")
    );

    assert_eq!(diagnostic.annotations().len(), 1);
    let annotation = &diagnostic.annotations()[0];
    assert_eq!(annotation.range, nth_range_of(source, "local", 1));
    assert_eq!(annotation.message, "conditional assignment to `local`");
}

#[test]
fn test_diagnostics_lowers_conditional_attach() {
    // `library(shiny)` attaches on only the `if` path, so it drops at the
    // join and `reactive()` reads as plain. `reactive`'s NSE annotation
    // comes from the static effect registry keyed on "shiny", which needs
    // `shiny` to resolve as an installed package (see `install_packages`);
    // unlike base, non-base packages aren't a hardcoded fallback.
    let source = "\
if (cond) library(shiny)
reactive({
    x <- 1
})
";
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let file = new_file(&mut db, "a.R", source);

    let diagnostics = file.diagnostics(&db);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];

    assert_eq!(diagnostic.kind(), DiagnosticKind::EffectAmbiguity);
    assert_eq!(
        diagnostic.message(),
        "Ambiguous reading of `reactive()`. The conditionally attached `shiny` \
         does not import `reactive` across all paths."
    );
    assert_eq!(
        diagnostic.range(),
        range_of(source, "reactive({\n    x <- 1\n})")
    );

    assert_eq!(diagnostic.annotations().len(), 1);
    let annotation = &diagnostic.annotations()[0];
    assert_eq!(annotation.range, range_of(source, "library(shiny)"));
    assert_eq!(annotation.message, "`shiny` attached only here");
}

#[test]
fn test_diagnostics_lowers_ambiguous_attach_order() {
    let source = "\
if (cond) {
    library(cli)
    library(rlang)
} else {
    library(rlang)
    library(cli)
}
";
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli", "rlang"]);
    let file = new_file(&mut db, "a.R", source);

    let diagnostics = file.diagnostics(&db);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];

    assert_eq!(diagnostic.kind(), DiagnosticKind::AmbiguousAttachOrder);
    assert_eq!(
        diagnostic.message(),
        "Ambiguous attach order. The branches attach `rlang`, `cli` in different orders, \
         so which package masks the other depends on the branch taken."
    );
    assert_eq!(diagnostic.range(), range_of(source, source.trim_end()));
    assert!(diagnostic.annotations().is_empty());
}
