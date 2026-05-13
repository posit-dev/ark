use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;
use salsa::Setter;

use crate::intern_file;
use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::FileOwner;
use crate::Root;
use crate::Script;

fn make_script(db: &mut TestDb, root: Root, name: &str, contents: &str) -> Script {
    let file = intern_file(db, file_url(name), contents.to_string(), None);
    let script = Script::new(db, root, file);
    file.set_owner(db).to(Some(FileOwner::Script(script)));
    script
}

/// Create a workspace root with the given scripts and register it.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> (Root, Vec<Script>) {
    let root = workspace_root(db, "");
    let scripts: Vec<Script> = scripts
        .iter()
        .map(|(name, contents)| make_script(db, root, name, contents))
        .collect();
    root.set_scripts(db).to(scripts.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    (root, scripts)
}

#[test]
fn cross_file_source_injection() {
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(
        &mut db,
        &[("a.R", "source(\"b.R\")\n"), ("b.R", "x <- 1\n")],
    );
    let (a, b) = (scripts[0], scripts[1]);

    let index = a.file(&db).semantic_index(&db);
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
            assert_eq!(file, b.file(&db).url(&db).as_url());
            assert_eq!(name, "x");
        },
        _ => unreachable!(),
    }
}

#[test]
fn editing_sourced_file_invalidates_caller_index() {
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(
        &mut db,
        &[("a.R", "source(\"b.R\")\n"), ("b.R", "x <- 1\n")],
    );
    let (a, b) = (scripts[0], scripts[1]);

    let _ = a.file(&db).semantic_index(&db);
    assert_eq!(db.executions("semantic_index"), 2);

    // Add a new top-level definition in `b`. `a` sees `b`'s exports
    // change, so its index must re-run.
    b.file(&db)
        .set_contents(&mut db)
        .to("x <- 1\ny <- 2\n".to_string());
    let _ = a.file(&db).semantic_index(&db);
    assert!(db.executions("semantic_index") >= 3);

    let index = a.file(&db).semantic_index(&db);
    let exports = index.file_exports();
    assert!(exports.contains_key("x"));
    assert!(exports.contains_key("y"));
}

#[test]
fn source_cycle_terminates_with_empty_index() {
    // `a` sources `b`, `b` sources `a`. Salsa breaks the cycle by
    // resolving one side to an empty index (the file scope only, no
    // definitions, no semantic calls).
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(
        &mut db,
        &[
            ("a.R", "source(\"b.R\")\nx_a <- 1\n"),
            ("b.R", "source(\"a.R\")\nx_b <- 2\n"),
        ],
    );
    let (a, b) = (scripts[0], scripts[1]);

    let index_a = a.file(&db).semantic_index(&db);
    let index_b = b.file(&db).semantic_index(&db);

    // Each non-empty index has its own top-level binding; the cycling
    // side is the empty cycle_result. We don't pin which side salsa
    // picks, only that at least one is the empty index and that builds
    // complete without panicking.
    let empty_a = index_a.file_exports().is_empty();
    let empty_b = index_b.file_exports().is_empty();
    assert!(empty_a || empty_b);
}

#[test]
fn closure_capture_with_source_before_function() {
    // source() comes first, so by the time `f`'s body is walked the
    // file-scope symbol table already has `helper` flagged
    // `IS_BOUND` via the injected Import. The free-variable lookup
    // inside `f` finds it through the existing enclosing-snapshot
    // machinery, no pre-scan needed.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(
        &mut db,
        &[
            ("script.R", "source(\"helpers.R\")\nf <- function() helper\n"),
            ("helpers.R", "helper <- 1\n"),
        ],
    );
    let script = scripts[0];

    let index = script.file(&db).semantic_index(&db);
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
#[ignore = "known limitation: pre-scan does not yet detect `source()` injection. \
            When source() follows the function definition, the function's free-variable \
            lookup runs before the Import lands in the file scope, so the enclosing \
            snapshot misses it. Fixing this requires extending the pre-scan to consult \
            the resolver. See 2026-05-13-1610-semantic-index-cross-file.md."]
fn closure_capture_with_source_after_function() {
    // Function defined first, source() injected after. The walk
    // processes `f`'s body before the source() call, so when
    // `register_enclosing_snapshot` looks up `helper` in the file
    // scope, neither the symbol table nor the pre-scan knows about it
    // yet, and the snapshot doesn't register.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(
        &mut db,
        &[
            ("script.R", "f <- function() helper\nsource(\"helpers.R\")\n"),
            ("helpers.R", "helper <- 1\n"),
        ],
    );
    let script = scripts[0];

    let index = script.file(&db).semantic_index(&db);
    let fn_scope = ScopeId::from(1);

    let use_id = oak_semantic::UseId::from(0);
    let symbol = index.uses(fn_scope)[use_id].symbol();
    assert!(index.enclosing_bindings(fn_scope, symbol).is_some());
}
