use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::Name;

/// Build a workspace root at `w` populated with the given files.
/// Returns the file handles in the same order. Registers the root with
/// `WorkspaceRoots` so `file_by_path` can find the files for cross-file
/// resolution.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let files: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| File::new(db, file_path(name), contents.to_string(), None))
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    files
}

fn name<'db>(db: &'db TestDb, text: &str) -> Name<'db> {
    Name::new(db, text)
}

#[test]
fn test_resolve_local_name_lands_on_owning_file() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];

    let def = file.resolve(&db, name(&db, "x")).expect("x should resolve");
    assert_eq!(def.file(&db), file);
    assert_eq!(def.name(&db).text(&db).as_str(), "x");
    // The name range is just the `x` identifier in `x <- 1`.
    let range = def.name_range(&db).expect("Local binding has a name range");
    assert_eq!(usize::from(range.start()), 0);
    assert_eq!(usize::from(range.end()), 1);
}

#[test]
fn test_unresolved_name_returns_none() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];
    assert!(file.resolve(&db, name(&db, "nope")).is_none());
}

#[test]
fn test_resolve_chases_source_forwarding_to_origin_file() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "helper <- function() 1\n"),
        ("w/analysis.R", "source(\"helpers.R\")\n"),
    ]);
    let helpers = files[0];
    let analysis = files[1];

    let def = analysis
        .resolve(&db, name(&db, "helper"))
        .expect("helper should resolve through source()");

    assert_eq!(def.file(&db), helpers);
    assert_eq!(def.name(&db).text(&db).as_str(), "helper");
}

#[test]
fn test_resolve_chases_two_step_source_chain() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/leaf.R", "deep <- 1\n"),
        ("w/mid.R", "source(\"leaf.R\")\n"),
        ("w/top.R", "source(\"mid.R\")\n"),
    ]);
    let leaf = files[0];
    let top = files[2];

    let def = top
        .resolve(&db, name(&db, "deep"))
        .expect("deep should chase through mid -> leaf");

    assert_eq!(def.file(&db), leaf);
    assert_eq!(def.name(&db).text(&db).as_str(), "deep");
}

#[test]
fn test_resolve_is_cached_across_repeat_calls() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];

    let _ = file.resolve(&db, name(&db, "x"));
    let _ = file.resolve(&db, name(&db, "x"));

    // Tracked: the second call hits the salsa cache. Match `resolve_(`
    // (salsa formats database keys as `File::resolve_(Id(...))`, with a
    // trailing underscore on the method name) so the substring doesn't
    // also pick up `resolve_export`, the tracked helper.
    assert_eq!(db.executions("resolve_("), 1);
}

#[test]
fn test_resolve_in_cyclic_source_returns_none_without_panicking() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "source(\"b.R\")\na_local <- 1\n"),
        ("w/b.R", "source(\"a.R\")\nb_local <- 2\n"),
    ]);
    let a = files[0];
    let b = files[1];

    // a.R sources b.R; b.R sources a.R. Both sides' `exports` cycle
    // to empty via `cycle_result`, so `resolve` returns `None` for
    // names that would otherwise be found in those exports. The
    // point of the test is that resolution terminates cleanly rather
    // than panicking.
    assert!(a.resolve(&db, name(&db, "a_local")).is_none());
    assert!(b.resolve(&db, name(&db, "b_local")).is_none());
}

/// Extract the source slice at `range` from `source`.
fn slice(source: &str, range: biome_rowan::TextRange) -> &str {
    &source[usize::from(range.start())..usize::from(range.end())]
}

#[test]
fn test_name_range_for_left_assignment() {
    let mut db = TestDb::new();
    let source = "x <- 1\n";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);
    let def = files[0]
        .resolve(&db, name(&db, "x"))
        .expect("x should resolve");
    let range = def.name_range(&db).expect("Local has name range");
    assert_eq!(slice(source, range), "x");
}

#[test]
fn test_name_range_for_right_assignment() {
    let mut db = TestDb::new();
    let source = "1 -> x\n";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);
    let def = files[0]
        .resolve(&db, name(&db, "x"))
        .expect("x should resolve");
    let range = def.name_range(&db).expect("Local has name range");
    assert_eq!(slice(source, range), "x");
}

#[test]
fn test_name_range_for_super_left_assignment() {
    let mut db = TestDb::new();
    let source = "x <<- 1\n";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);
    let def = files[0]
        .resolve(&db, name(&db, "x"))
        .expect("x should resolve");
    let range = def.name_range(&db).expect("Local has name range");
    assert_eq!(slice(source, range), "x");
}

#[test]
fn test_name_range_for_super_right_assignment() {
    let mut db = TestDb::new();
    let source = "1 ->> x\n";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);
    let def = files[0]
        .resolve(&db, name(&db, "x"))
        .expect("x should resolve");
    let range = def.name_range(&db).expect("Local has name range");
    assert_eq!(slice(source, range), "x");
}

#[test]
fn test_name_range_for_string_as_name() {
    // R's `"x" <- 1` binds `x`. The LHS in the parse tree is an
    // `RStringValue`, not an `RIdentifier`. The range covers the quoted
    // string literal.
    let mut db = TestDb::new();
    let source = "\"x\" <- 1\n";
    let files = setup_workspace(&mut db, &[("w/a.R", source)]);
    let def = files[0]
        .resolve(&db, name(&db, "x"))
        .expect("x should resolve");
    let range = def.name_range(&db).expect("Local has name range");
    assert_eq!(slice(source, range), "\"x\"");
}

#[test]
fn test_name_range_returns_none_for_import_kind() {
    // `File::resolve` chases past every `Import` and only ever returns a
    // `Local`-kinded `Definition`, so the `Import` arm of `name_range` is
    // unreachable from the public resolution API. Wrap construction of an
    // `Import` definition in a tracked helper (salsa requires tracked
    // structs be created on the query stack) and exercise the arm directly.
    use aether_syntax::RCall;
    use biome_rowan::AstNode;
    use biome_rowan::AstPtr;
    use oak_semantic::semantic_index::DefinitionKind;
    use oak_semantic::semantic_index::ScopeId;

    use crate::Db;
    use crate::Definition;

    #[salsa::tracked]
    fn build_import_definition<'db>(
        db: &'db dyn Db,
        file: File,
        name: Name<'db>,
    ) -> Definition<'db> {
        let parse = file.parse(db);
        let call = parse
            .tree()
            .syntax()
            .descendants()
            .find_map(RCall::cast)
            .expect("file must contain a call");
        let kind = DefinitionKind::Import {
            call: AstPtr::new(&call),
            file: file.path(db).to_url(),
            name: name.text(db).to_string(),
        };
        Definition::new(db, file, ScopeId::from(0), name, kind)
    }

    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "source(\"b.R\")\n")]);
    let def = build_import_definition(&db, files[0], name(&db, "foo"));

    assert!(def.name_range(&db).is_none());
}

#[test]
fn test_definition_id_stable_across_body_edits() {
    // The headline claim of `Definition` being a salsa-tracked entity with
    // `(file, scope, name)` identity: a body edit that shifts the binding's
    // source position must produce a `Definition` with the same salsa id.
    // Only the volatile `range` field changes between revisions; consumers
    // that depend on identity stay cached.
    use salsa::plumbing::AsId;

    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];

    // Capture the salsa id and range out of the entity before mutating db,
    // since the `Definition<'db>` borrow conflicts with `set_contents`'s
    // mutable borrow.
    let (id1, range1) = {
        let def = file.resolve(&db, name(&db, "x")).expect("x should resolve");
        (def.as_id(), def.name_range(&db))
    };

    // Add a function above `x`, shifting its position downward.
    file.set_contents(&mut db)
        .to("f <- function() 2\nx <- 1\n".to_string());

    let (id2, range2) = {
        let def = file
            .resolve(&db, name(&db, "x"))
            .expect("x should still resolve");
        (def.as_id(), def.name_range(&db))
    };

    // Same salsa entity across the edit: identity tuple unchanged.
    assert_eq!(id1, id2);
    // Range moved (the binding is now on line 2).
    assert_ne!(range1, range2);
}

#[test]
fn test_definition_id_stable_across_def_id_renumber_local_path() {
    // The function-scope local path looks up its `Definition` from the single
    // mint site `File::definitions`, whose identity is `(file, scope, name)`.
    // `def_id` is only the lookup key, never part of identity, so prepending
    // an unrelated binding inside the function (which renumbers x's `def_id`)
    // leaves x's salsa id unchanged. This is the same stability the export
    // path has in `test_definition_id_stable_across_body_edits`, now extended
    // to the local path.
    use biome_rowan::TextSize;
    use salsa::plumbing::AsId;

    let content1 = "f <- function() {\nx <- 1\nx\n}\n";
    let use1 = content1.find("\nx\n").expect("standalone use of x") + 1;

    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", content1)]);
    let file = files[0];

    let id1 = {
        let defs = file.resolve_at(&db, TextSize::from(use1 as u32));
        assert_eq!(defs.len(), 1);
        defs[0].as_id()
    };

    // Prepend an unrelated binding inside the function so x's DefinitionId
    // shifts 0 -> 1 within the function scope.
    let content2 = "f <- function() {\nw <- 0\nx <- 1\nx\n}\n";
    let use2 = content2.find("\nx\n").expect("standalone use of x") + 1;
    file.set_contents(&mut db).to(content2.to_string());

    let id2 = {
        let defs = file.resolve_at(&db, TextSize::from(use2 as u32));
        assert_eq!(defs.len(), 1);
        defs[0].as_id()
    };

    assert_eq!(id1, id2);
}

#[test]
fn test_definitions_mints_distinct_entities_for_same_name_redefinition() {
    // Two file-scope `x` bindings share the `(file, scope, name)` id-fields.
    // The single mint site must create two distinct salsa entities rather than
    // collide or panic; salsa disambiguates same-id-field tracked structs by
    // creation order. Resolving `x` forces the mint of both (via `definitions`)
    // and must land on the last definition (offset 7).
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\nx <- 2\n")]);
    let file = files[0];

    let def = file.resolve(&db, name(&db, "x")).expect("x should resolve");
    assert_eq!(def.name(&db).text(&db).as_str(), "x");
    let range = def.name_range(&db).expect("local binding has a name range");
    assert_eq!(usize::from(range.start()), 7);
}

#[test]
fn test_position_shift_keeps_id_and_does_not_invalidate_identity_consumers() {
    // A pure position shift (prepend a comment, no binding added or removed)
    // moves the binding's AstPtr but leaves `(file, scope, name)` and its
    // ordinal unchanged, so the salsa id is stable. A downstream query that
    // reads only identity therefore stays cached across the rebuild; only
    // consumers of `kind` (the moved AstPtr) would re-run.
    use salsa::plumbing::AsId;

    use crate::Db;
    use crate::Definition;

    #[salsa::tracked]
    fn name_len<'db>(db: &'db dyn Db, def: Definition<'db>) -> usize {
        def.name(db).text(db).len()
    }

    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];

    let (id1, range1) = {
        let def = file.resolve(&db, name(&db, "x")).expect("x resolves");
        let _ = name_len(&db, def);
        (def.as_id(), def.name_range(&db))
    };
    assert_eq!(db.executions("name_len"), 1);

    // Pure position shift: x moves down a line, no binding added or removed.
    file.set_contents(&mut db)
        .to("# comment\nx <- 1\n".to_string());

    let (id2, range2) = {
        let def = file.resolve(&db, name(&db, "x")).expect("x still resolves");
        let _ = name_len(&db, def);
        (def.as_id(), def.name_range(&db))
    };

    // Same entity, and the name range moved (it really was a position shift).
    assert_eq!(id1, id2);
    assert_ne!(range1, range2);
    // The identity-only consumer was not re-executed by the position shift.
    assert_eq!(db.executions("name_len"), 1);
}

#[test]
fn test_same_name_sibling_insertion_churns_later_definition_id() {
    // TRACKING TEST for a known boundary, not a guarantee to preserve.
    //
    // Identity is `(file, scope, name)` plus salsa's creation-order
    // disambiguator among same-name siblings. Inserting *another* `x` earlier
    // in the scope shifts the ordinals of the later `x` definitions, so their
    // salsa ids churn even though their position-stability would otherwise
    // hold. This matches ty's `push_additional_definition` ordering. The test
    // exists to notice if salsa's disambiguation ever changes.
    use salsa::plumbing::AsId;

    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\nx <- 2\n")]);
    let file = files[0];

    // `resolve` is last-wins, so it returns the final `x` (`x <- 2`), ordinal 1.
    let id1 = file
        .resolve(&db, name(&db, "x"))
        .expect("x resolves")
        .as_id();

    // Insert another `x` at the top. The final `x` is still last-wins, but its
    // ordinal among same-name siblings shifts from 1 to 2.
    file.set_contents(&mut db)
        .to("x <- 0\nx <- 1\nx <- 2\n".to_string());

    let id2 = file
        .resolve(&db, name(&db, "x"))
        .expect("x still resolves")
        .as_id();

    assert_ne!(id1, id2);
}

#[test]
fn test_resolve_unbound_name_in_package_does_not_cycle() {
    // Without exports-only sibling chase, A's `resolve` would walk into
    // B's `resolve`, which would walk back into A via B's imports
    // (sibling exclusion is per-file), and salsa would panic on the
    // unbound name. Test that we return None cleanly.
    let mut db = TestDb::new();
    let workspace = workspace_root(&db, "w/pkg");
    let pkg = crate::Package::new(
        &db,
        file_path("/w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        None,
        oak_package_metadata::namespace::Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );

    let a = File::new(
        &db,
        file_path("/w/pkg/R/a.R"),
        "x <- 1\n".to_string(),
        Some(pkg),
    );
    let b = File::new(
        &db,
        file_path("/w/pkg/R/b.R"),
        "y <- 2\n".to_string(),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![a, b]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert!(a.resolve(&db, name(&db, "nope")).is_none());
    assert!(b.resolve(&db, name(&db, "nope")).is_none());
}

#[test]
fn test_resolve_walks_package_files_for_lazy_lookups() {
    // `resolve` is the lazy / EOF-state lookup. By the time a function
    // in `b.R` runs, the whole package has been sourced, so `b.R`'s
    // function bodies see definitions from any sibling file. Test
    // directly on `resolve` (not `resolve_at`) to nail down the
    // imports walk.
    let mut db = TestDb::new();
    let workspace = workspace_root(&db, "w/pkg");
    let pkg = crate::Package::new(
        &db,
        file_path("/w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        None,
        oak_package_metadata::namespace::Namespace::default(),
        Vec::new(),
        Vec::new(),
        None,
    );

    let a = File::new(
        &db,
        file_path("/w/pkg/R/a.R"),
        "shared <- 1\n".to_string(),
        Some(pkg),
    );
    let b = File::new(
        &db,
        file_path("/w/pkg/R/b.R"),
        "use_shared <- function() shared\n".to_string(),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![a, b]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    // `b` has no top-level `shared`, but `a` (a sibling file in the
    // same package) does. `b.resolve("shared")` should find it via the
    // imports walk.
    let def = b.resolve(&db, name(&db, "shared")).expect("should resolve");
    assert_eq!(def.file(&db), a);
    assert_eq!(def.name(&db).text(&db).as_str(), "shared");
}
