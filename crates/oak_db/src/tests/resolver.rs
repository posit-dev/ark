use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticCallKind;
use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;
use crate::Root;

fn make_script(db: &mut TestDb, name: &str, contents: &str) -> File {
    File::new(db, file_url(name), contents.to_string(), None)
}

/// Build a fresh workspace root, attach the given scripts, register
/// it on the singleton `WorkspaceRoots` input.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> (Root, Vec<File>) {
    let root = workspace_root(db, "");
    let scripts: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| make_script(db, name, contents))
        .collect();
    root.set_scripts(db).to(scripts.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    (root, scripts)
}

#[test]
fn test_cross_file_source_injection() {
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "x <- 1\n"),
    ]);
    let (a, b) = (scripts[0], scripts[1]);

    let index = a.semantic_index(&db);
    let file_scope = ScopeId::from(0);

    let exports = index.file_exports();
    assert!(exports.contains_key("x"));

    let import_def = index
        .definitions(file_scope)
        .iter()
        .find(|(_, def)| matches!(def.kind(), DefinitionKind::Import { .. }));
    assert!(import_def.is_some());

    match import_def.unwrap().1.kind() {
        DefinitionKind::Import { file, name, .. } => {
            assert_eq!(file, b.url(&db).as_url());
            assert_eq!(name, "x");
        },
        _ => unreachable!(),
    }
}

#[test]
fn test_editing_sourced_file_invalidates_caller_index() {
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "x <- 1\n"),
    ]);
    let (a, b) = (scripts[0], scripts[1]);

    let _ = a.semantic_index(&db);
    assert_eq!(db.executions("semantic_index"), 2);

    // Add a new top-level definition in `b`. `a` sees `b`'s exports
    // change, so its index must re-run.
    b.set_contents(&mut db).to("x <- 1\ny <- 2\n".to_string());
    let _ = a.semantic_index(&db);
    // 4 = 2 initial (a + b) + 2 re-runs (b's parse and index invalidate
    // first via the contents bump, then a's index re-runs because its
    // dep on b's index lost validity).
    assert_eq!(db.executions("semantic_index"), 4);

    let index = a.semantic_index(&db);
    let exports = index.file_exports();
    assert!(exports.contains_key("x"));
    assert!(exports.contains_key("y"));
}

#[test]
fn test_source_cycle_preserves_local_analysis() {
    // `a` sources `b`, `b` sources `a`. Salsa breaks the cycle by
    // rebuilding one side with `NoopImportsResolver`, so that side keeps its
    // own local definitions but loses the cross-file imports from the
    // cycle partner. The other side completes normally.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\nx_a <- 1\n"),
        ("b.R", "source(\"a.R\")\nx_b <- 2\n"),
    ]);
    let (a, b) = (scripts[0], scripts[1]);

    let index_a = a.semantic_index(&db);
    let index_b = b.semantic_index(&db);

    // Both files keep their own local binding regardless of which side
    // salsa picks as the cycle break point.
    assert!(index_a.file_exports().contains_key("x_a"));
    assert!(index_b.file_exports().contains_key("x_b"));
}

#[test]
fn test_closure_capture_with_source_before_function() {
    // source() comes first, so by the time `f`'s body is walked the
    // file-scope symbol table already has `helper` flagged
    // `IS_BOUND` via the injected Import. The free-variable lookup
    // inside `f` finds it through the existing enclosing-snapshot
    // machinery, no pre-scan needed.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        (
            "script.R",
            "source(\"helpers.R\")\nf <- function() helper\n",
        ),
        ("helpers.R", "helper <- 1\n"),
    ]);
    let script = scripts[0];

    let index = script.semantic_index(&db);
    let file_scope = ScopeId::from(0);
    let fn_scope = ScopeId::from(1);

    // The function body's lone use is `helper`. Its enclosing snapshot
    // should point at the file scope and the bindings should be
    // non-empty (containing the Import).
    let fn_map = index.use_def_map(fn_scope);
    let use_id = oak_semantic::UseId::from(0);
    let bindings = fn_map.bindings_at_use(use_id);
    assert!(bindings.may_be_unbound());

    let symbol = index.uses(fn_scope)[use_id].symbol();
    let (enclosing_scope, enclosing_bindings) = index
        .enclosing_bindings(fn_scope, symbol)
        .expect("`helper` should have an enclosing snapshot at the file scope");
    assert_eq!(enclosing_scope, file_scope);
    assert!(!enclosing_bindings.definitions().is_empty());

    // The enclosing binding is the Import injected by source().
    let def_id = enclosing_bindings.definitions()[0];
    let def = &index.definitions(file_scope)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
}

#[test]
fn test_sourced_file_library_attaches_in_caller() {
    // `b.R` calls `library(foo)`. After `a.R` sources `b.R`, the
    // resolver carries `foo` into `a`'s attached-packages set, so a
    // scope query against `a` sees the same packages it would see if
    // the `library(foo)` call had appeared directly in `a`.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "library(foo)\n"),
    ]);
    let a = scripts[0];

    let index = a.semantic_index(&db);
    assert!(index.file_attached_packages().contains(&"foo"));
}

#[test]
fn test_source_to_unregistered_url_resolves_to_none() {
    // `a.R` sources `b.R` but `b.R` isn't registered. The `Source`
    // semantic call is still recorded so diagnostics can flag the
    // unresolved import; no `Import` definition lands in `a`'s file
    // scope.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[("a.R", "source(\"b.R\")\n")]);
    let a = scripts[0];

    let index = a.semantic_index(&db);

    let imports = index
        .definitions(ScopeId::from(0))
        .iter()
        .any(|(_, def)| matches!(def.kind(), DefinitionKind::Import { .. }));
    assert!(!imports);

    let source_calls: Vec<_> = index
        .semantic_calls()
        .iter()
        .filter_map(|c| match c.kind() {
            SemanticCallKind::Source { path, resolved } => Some((path.as_str(), resolved)),
            _ => None,
        })
        .collect();
    assert_eq!(source_calls, [("b.R", &None)]);
}

#[test]
fn test_source_resolves_absolute_path() {
    // `source("/abs/b.R")` joins to an absolute path regardless of
    // `a`'s parent directory. The target URL is reconstructed
    // unambiguously and the registered script at that URL is found.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"/abs/b.R\")\n"),
        ("abs/b.R", "x <- 1\n"),
    ]);
    let a = scripts[0];

    let index = a.semantic_index(&db);
    assert!(index.file_exports().contains_key("x"));
}

#[test]
fn test_source_chain_propagates_exports_transitively() {
    // a sources b, b sources c, c defines x_c. Each Import is recorded
    // at its `source()` call site, and `file_exports` walks them all
    // out, so a sees x_a, x_b (forwarded from b), and x_c (forwarded
    // from b which forwarded it from c).
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\nx_a <- 1\n"),
        ("b.R", "source(\"c.R\")\nx_b <- 2\n"),
        ("c.R", "x_c <- 3\n"),
    ]);
    let a = scripts[0];

    let exports = a.semantic_index(&db).file_exports();
    assert!(exports.contains_key("x_a"));
    assert!(exports.contains_key("x_b"));
    assert!(exports.contains_key("x_c"));
}

#[test]
#[ignore = "known limitation: pre-scan does not yet detect `source()` injection. \
            When source() follows the function definition, the function's free-variable \
            lookup runs before the Import lands in the file scope, so the enclosing \
            snapshot misses it. Fixing this requires extending the pre-scan to consult \
            the resolver for source() / library() targets -- the same extension NSE \
            scope resolution needs to detect imported NSE call targets that are \
            brought in by source() / library() later in the file. TODO(nse)"]
fn test_closure_capture_with_source_after_function() {
    // Function defined first, source() injected after. The walk
    // processes `f`'s body before the source() call, so when
    // `register_enclosing_snapshot` looks up `helper` in the file
    // scope, neither the symbol table nor the pre-scan knows about it
    // yet, and the snapshot doesn't register.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        (
            "script.R",
            "f <- function() helper\nsource(\"helpers.R\")\n",
        ),
        ("helpers.R", "helper <- 1\n"),
    ]);
    let script = scripts[0];

    let index = script.semantic_index(&db);
    let fn_scope = ScopeId::from(1);

    let use_id = oak_semantic::UseId::from(0);
    let symbol = index.uses(fn_scope)[use_id].symbol();
    assert!(index.enclosing_bindings(fn_scope, symbol).is_some());
}
