use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use anyhow::anyhow;
use oak_ide::NavigationTarget;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::LocationLink;

use crate::lsp::document::Document;
use crate::lsp::state::WorldState;

pub(crate) fn goto_definition(
    document: &Document,
    params: GotoDefinitionParams,
    state: &WorldState,
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;

    let (index, file_scope) = state.file_analysis(&uri, document);
    let root = document.syntax();
    let targets =
        oak_ide::goto_definition(offset, &uri, &root, &index, &file_scope, &state.library);

    if targets.is_empty() {
        return Ok(None);
    }

    let links = targets
        .into_iter()
        .map(|target| nav_target_to_link(target, state))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(GotoDefinitionResponse::Link(links)))
}

fn nav_target_to_link(
    target: NavigationTarget,
    state: &WorldState,
) -> anyhow::Result<LocationLink> {
    let doc = if let Some(open) = state.documents.get(&target.file) {
        open
    } else {
        let path = target
            .file
            .to_file_path()
            .map_err(|_| anyhow!("Can't convert URI to path: {}", target.file))?;
        let contents = std::fs::read_to_string(&path)?;
        &Document::new(&contents, None)
    };

    let target_range = to_proto::range(target.full_range, &doc.line_index, doc.position_encoding)?;
    let target_selection_range =
        to_proto::range(target.focus_range, &doc.line_index, doc.position_encoding)?;

    Ok(LocationLink {
        origin_selection_range: None,
        target_uri: target.file,
        target_range,
        target_selection_range,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use assert_matches::assert_matches;
    use oak_package::library::Library;
    use oak_package::package::Package;
    use oak_package::package_description::Description;
    use oak_package::package_namespace::Import;
    use oak_package::package_namespace::Namespace;
    use tower_lsp::lsp_types;

    use super::*;
    use crate::lsp::document::Document;
    use crate::lsp::inputs::source_root::SourceRoot;
    use crate::lsp::util::test_path;

    fn r_library() -> Option<Library> {
        let output = Command::new("R")
            .args(["--no-save", "-e", "cat(.libPaths(), sep='\\n')"])
            .output()
            .ok()?;
        let stdout = String::from_utf8(output.stdout).ok()?;
        let paths: Vec<PathBuf> = stdout.lines().map(PathBuf::from).collect();
        if paths.is_empty() {
            return None;
        }
        Some(Library::new(paths))
    }

    fn make_params(uri: lsp_types::Url, line: u32, character: u32) -> GotoDefinitionParams {
        GotoDefinitionParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    fn make_state(uri: &lsp_types::Url, doc: &Document) -> WorldState {
        let mut state = WorldState::default();
        state.documents.insert(uri.clone(), doc.clone());
        state
    }

    #[test]
    fn test_goto_definition() {
        let code = "foo <- 42\nprint(foo)\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");
        let state = make_state(&uri, &doc);

        let params = make_params(uri, 1, 6);

        assert_matches!(
            goto_definition(&doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 3),
                    }
                );
            }
        );
    }

    #[test]
    fn test_goto_definition_prefers_local_symbol() {
        let code = "foo <- 1\nfoo\n";
        let doc = Document::new(code, None);
        let uri = test_path("file.R");
        let state = make_state(&uri, &doc);

        let params = make_params(uri.clone(), 1, 0);

        assert_matches!(
            goto_definition(&doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 3),
                    }
                );
            }
        );
    }

    #[test]
    fn test_goto_definition_no_use_returns_none() {
        let code = "x <- 1\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");
        let state = make_state(&uri, &doc);

        let params = make_params(uri, 0, 3);
        let result = goto_definition(&doc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_goto_definition_unresolved_returns_none() {
        let code = "foo\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");
        let state = make_state(&uri, &doc);

        let params = make_params(uri, 0, 0);
        let result = goto_definition(&doc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_other_file_not_visible_without_scope_chain() {
        // file2 uses `foo` but file1's definition is not in the scope chain,
        // so it should not resolve.
        let doc1 = Document::new("foo <- 1\n", None);
        let uri1 = test_path("file1.R");

        let doc2 = Document::new("foo\n", None);
        let uri2 = test_path("file2.R");

        let mut state = WorldState::default();
        state.documents.insert(uri1, doc1);
        state.documents.insert(uri2.clone(), doc2.clone());

        let params = make_params(uri2, 0, 0);
        let result = goto_definition(&doc2, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_package_import_from_resolves() {
        // A package with `importFrom(dplyr, mutate)` should make `mutate`
        // visible. But we don't have a file/range for package symbols yet,
        // so the result is None.
        let doc = Document::new("mutate\n", None);
        let uri = test_path("R/my_file.R");

        let ns = Namespace {
            imports: vec![Import {
                name: "mutate".to_string(),
                package: "dplyr".to_string(),
            }],
            ..Default::default()
        };
        let desc = Description {
            name: "mypkg".to_string(),
            ..Default::default()
        };
        let pkg = Package::from_parts(PathBuf::from("/fake"), desc, ns);

        let mut state = make_state(&uri, &doc);
        state.root = Some(SourceRoot::Package(pkg));

        let params = make_params(uri, 0, 0);
        let result = goto_definition(&doc, params, &state).unwrap();
        // Package symbols don't produce NavigationTargets yet
        assert_eq!(result, None);
    }

    #[test]
    fn test_cross_file_via_collation() {
        // Collation order: aaa.R, bbb.R, ccc.R
        // bbb.R defines `helper`. ccc.R (later) can see it,
        // aaa.R (earlier) cannot.
        let pkg_root = std::env::temp_dir().join("test_pkg");

        let doc_aaa = Document::new("helper\n", None);
        let uri_aaa = lsp_types::Url::from_file_path(pkg_root.join("R/aaa.R")).unwrap();

        let doc_bbb = Document::new("helper <- function() 1\n", None);
        let uri_bbb = lsp_types::Url::from_file_path(pkg_root.join("R/bbb.R")).unwrap();

        let doc_ccc = Document::new("helper\n", None);
        let uri_ccc = lsp_types::Url::from_file_path(pkg_root.join("R/ccc.R")).unwrap();

        let ns = Namespace::default();
        let desc = Description {
            name: "mypkg".to_string(),
            ..Default::default()
        };
        let pkg = Package::from_parts(pkg_root, desc, ns);

        let mut state = WorldState::default();
        state.documents.insert(uri_aaa.clone(), doc_aaa.clone());
        state.documents.insert(uri_bbb.clone(), doc_bbb);
        state.documents.insert(uri_ccc.clone(), doc_ccc.clone());
        state.root = Some(SourceRoot::Package(pkg));

        // ccc.R sees bbb.R's definition (later in collation)
        let params = make_params(uri_ccc, 0, 0);
        assert_matches!(
            goto_definition(&doc_ccc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, uri_bbb);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 6),
                    }
                );
            }
        );

        // aaa.R cannot see bbb.R's definition (earlier in collation)
        let params = make_params(uri_aaa, 0, 0);
        let result = goto_definition(&doc_aaa, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_cross_file_collate_field_reverses_order() {
        // Same files, but DESCRIPTION has `Collate: ccc.R bbb.R aaa.R`
        // which reverses the order. Now aaa.R is last, so it can see
        // bbb.R's definition. ccc.R is first, so it cannot.
        let pkg_root = std::env::temp_dir().join("test_pkg_collate");

        let doc_aaa = Document::new("helper\n", None);
        let uri_aaa = lsp_types::Url::from_file_path(pkg_root.join("R/aaa.R")).unwrap();

        let doc_bbb = Document::new("helper <- function() 1\n", None);
        let uri_bbb = lsp_types::Url::from_file_path(pkg_root.join("R/bbb.R")).unwrap();

        let doc_ccc = Document::new("helper\n", None);
        let uri_ccc = lsp_types::Url::from_file_path(pkg_root.join("R/ccc.R")).unwrap();

        let mut dcf_fields = std::collections::HashMap::new();
        dcf_fields.insert("Collate".to_string(), "ccc.R bbb.R aaa.R".to_string());

        let ns = Namespace::default();
        let desc = Description {
            name: "mypkg".to_string(),
            fields: oak_package::Dcf { fields: dcf_fields },
            ..Default::default()
        };
        let pkg = Package::from_parts(pkg_root, desc, ns);

        let mut state = WorldState::default();
        state.documents.insert(uri_aaa.clone(), doc_aaa.clone());
        state.documents.insert(uri_bbb.clone(), doc_bbb);
        state.documents.insert(uri_ccc.clone(), doc_ccc.clone());
        state.root = Some(SourceRoot::Package(pkg));

        // aaa.R is now last in collation, so it can see bbb.R's definition
        let params = make_params(uri_aaa, 0, 0);
        assert_matches!(
            goto_definition(&doc_aaa, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, uri_bbb);
            }
        );

        // ccc.R is now first in collation, so it cannot see bbb.R's definition
        let params = make_params(uri_ccc, 0, 0);
        let result = goto_definition(&doc_ccc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_use_in_function_body_resolves_across_files() {
        // aaa.R uses `helper` inside a function body. zzz.R defines
        // `helper` at the top level. In alphabetical collation zzz.R
        // comes after aaa.R, but the definition should still be visible
        // because function bodies execute lazily — the full package
        // namespace is populated before any function is called.
        let pkg_root = std::env::temp_dir().join("test_pkg_lazy");

        let doc_aaa = Document::new("f <- function() helper()\n", None);
        let uri_aaa = lsp_types::Url::from_file_path(pkg_root.join("R/aaa.R")).unwrap();

        let doc_zzz = Document::new("helper <- function() 1\n", None);
        let uri_zzz = lsp_types::Url::from_file_path(pkg_root.join("R/zzz.R")).unwrap();

        let ns = Namespace::default();
        let desc = Description {
            name: "mypkg".to_string(),
            ..Default::default()
        };
        let pkg = Package::from_parts(pkg_root, desc, ns);

        let mut state = WorldState::default();
        state.documents.insert(uri_aaa.clone(), doc_aaa.clone());
        state.documents.insert(uri_zzz.clone(), doc_zzz);
        state.root = Some(SourceRoot::Package(pkg));

        // Cursor on `helper` inside the function body (line 0, col 16)
        let params = make_params(uri_aaa, 0, 16);
        assert_matches!(
            goto_definition(&doc_aaa, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, uri_zzz);
            }
        );
    }

    // --- Base R and search path ---

    #[test]
    fn test_package_scope_includes_base() {
        // `cat` is in the base INDEX, so it resolves through the real
        // library. file_scope adds base at the bottom of the package chain.
        let Some(library) = r_library() else {
            eprintln!("skipping: R not found");
            return;
        };

        let pkg_root = std::env::temp_dir().join("test_pkg_base");

        let doc = Document::new("cat(1)\n", None);
        let uri = lsp_types::Url::from_file_path(pkg_root.join("R/foo.R")).unwrap();

        let ns = Namespace::default();
        let desc = Description {
            name: "mypkg".to_string(),
            ..Default::default()
        };
        let pkg = Package::from_parts(pkg_root, desc, ns);

        let mut state = make_state(&uri, &doc);
        state.root = Some(SourceRoot::Package(pkg));
        state.library = library;

        // `cat` at file level — package symbol, no NavigationTarget yet
        let params = make_params(uri, 0, 0);
        let result = goto_definition(&doc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_search_path_includes_defaults() {
        // A script outside a package should see base symbols via the
        // default search path built by file_scope.
        let Some(library) = r_library() else {
            eprintln!("skipping: R not found");
            return;
        };

        let doc = Document::new("cat(1)\n", None);
        let uri = test_path("script.R");

        let mut state = make_state(&uri, &doc);
        state.library = library;

        let params = make_params(uri, 0, 0);
        let result = goto_definition(&doc, params, &state).unwrap();
        // Package symbol, no NavigationTarget yet
        assert_eq!(result, None);
    }

    #[test]
    fn test_search_path_same_for_top_level_and_function() {
        // Unlike packages, scripts use the same scope chain everywhere.
        let Some(library) = r_library() else {
            eprintln!("skipping: R not found");
            return;
        };

        let code = "f <- function() cat(1)\ncat(2)\n";
        let doc = Document::new(code, None);
        let uri = test_path("script.R");

        let mut state = make_state(&uri, &doc);
        state.library = library;

        // `cat` at top level (line 1, col 0)
        let params = make_params(uri.clone(), 1, 0);
        let top_result = goto_definition(&doc, params, &state).unwrap();

        // `cat` inside function body (line 0, col 16)
        let params = make_params(uri, 0, 16);
        let fn_result = goto_definition(&doc, params, &state).unwrap();

        // Both resolve the same way (both None since package symbols
        // don't produce NavigationTargets yet)
        assert_eq!(top_result, None);
        assert_eq!(fn_result, None);
    }

    #[test]
    fn test_fixme_base_function_missing_from_index() {
        // `is.null` is a base R function but it's not in the base INDEX
        // file (it's documented under the `NULL` help page). Since base
        // has no NAMESPACE, we rely on INDEX for exported symbols, which
        // misses many common functions.
        let Some(library) = r_library() else {
            eprintln!("skipping: R not found");
            return;
        };

        let pkg_root = std::env::temp_dir().join("test_pkg_base_fixme");

        let ns = Namespace::default();
        let desc = Description {
            name: "mypkg".to_string(),
            ..Default::default()
        };
        let pkg = Package::from_parts(pkg_root.clone(), desc, ns);

        // `cat` IS in the INDEX — verify it resolves (no NavigationTarget
        // for package symbols, but the scope chain finds it)
        let doc_cat = Document::new("cat(1)\n", None);
        let uri_cat = lsp_types::Url::from_file_path(pkg_root.join("R/foo.R")).unwrap();

        let mut state = make_state(&uri_cat, &doc_cat);
        state.root = Some(SourceRoot::Package(pkg));
        state.library = library;

        let params = make_params(uri_cat, 0, 0);
        let result = goto_definition(&doc_cat, params, &state).unwrap();
        assert_eq!(result, None); // package symbol, no NavigationTarget yet

        // `is.null` is NOT in the INDEX
        let doc_null = Document::new("is.null(1)\n", None);
        let uri_null = lsp_types::Url::from_file_path(pkg_root.join("R/bar.R")).unwrap();
        state.documents.insert(uri_null.clone(), doc_null.clone());

        let params = make_params(uri_null, 0, 0);
        let result = goto_definition(&doc_null, params, &state).unwrap();
        // FIXME: should resolve to base::is.null but doesn't because
        // `is.null` is missing from the INDEX-based export list.
        assert_eq!(result, None);
    }

    // --- source() directive in scripts ---

    #[test]
    fn test_script_source_directive_resolves() {
        // script.R has `source("helpers.R")` then uses `helper`.
        // WorldState::file_scope() should resolve the source() directive
        // and make helpers.R's exports visible via the search path.
        let script_dir = std::env::temp_dir().join("test_script_source");

        let helpers_doc = Document::new("helper <- function() 1\n", None);
        let helpers_uri = lsp_types::Url::from_file_path(script_dir.join("helpers.R")).unwrap();

        let script_doc = Document::new("source(\"helpers.R\")\nhelper\n", None);
        let script_uri = lsp_types::Url::from_file_path(script_dir.join("script.R")).unwrap();

        let mut state = WorldState::default();
        state.documents.insert(helpers_uri.clone(), helpers_doc);
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        // Cursor on `helper` (line 1, col 0)
        let params = make_params(script_uri, 1, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, helpers_uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 6),
                    }
                );
            }
        );
    }

    #[test]
    fn test_script_source_directive_resolves_nested_library() {
        // helpers.R has `library(dplyr)` and defines `helper`.
        // script.R sources helpers.R then uses `mutate` (from dplyr).
        // The nested library() directive should be visible through
        // the source() resolution.
        let Some(library) = r_library() else {
            eprintln!("skipping: R not found");
            return;
        };

        let script_dir = std::env::temp_dir().join("test_script_source_nested");

        let helpers_doc = Document::new("library(dplyr)\nhelper <- function() 1\n", None);
        let helpers_uri = lsp_types::Url::from_file_path(script_dir.join("helpers.R")).unwrap();

        let script_doc = Document::new("source(\"helpers.R\")\nmutate\nhelper\n", None);
        let script_uri = lsp_types::Url::from_file_path(script_dir.join("script.R")).unwrap();

        let mut state = make_state(&script_uri, &script_doc);
        state.library = library;
        state.documents.insert(helpers_uri.clone(), helpers_doc);

        // `mutate` (line 1) resolves via dplyr, attached by helpers.R's library() call.
        // Package symbol, no NavigationTarget.
        let params = make_params(script_uri.clone(), 1, 0);
        let result = goto_definition(&script_doc, params, &state).unwrap();
        assert_eq!(result, None);

        // `helper` (line 2) resolves to helpers.R's definition
        let params = make_params(script_uri, 2, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, helpers_uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(1, 0),
                        end: lsp_types::Position::new(1, 6),
                    }
                );
            }
        );
    }

    #[test]
    fn test_script_source_resolves_from_disk() {
        // helpers.R exists on disk but is NOT in state.documents.
        // script.R sources it. The resolver should read from disk.
        let script_dir = tempfile::tempdir().unwrap();

        let helpers_path = script_dir.path().join("helpers.R");
        std::fs::write(&helpers_path, "helper <- function() 1\n").unwrap();
        let helpers_uri = lsp_types::Url::from_file_path(&helpers_path).unwrap();

        let script_doc = Document::new("source(\"helpers.R\")\nhelper\n", None);
        let script_uri =
            lsp_types::Url::from_file_path(script_dir.path().join("script.R")).unwrap();

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());
        // helpers.R is intentionally NOT inserted into state.documents

        let params = make_params(script_uri, 1, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, helpers_uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 6),
                    }
                );
            }
        );
    }

    #[test]
    fn test_script_file_scope_from_disk() {
        // The script itself is on disk, not in state.documents.
        // file_scope should still read it and resolve its directives.
        let script_dir = tempfile::tempdir().unwrap();

        let helpers_path = script_dir.path().join("helpers.R");
        std::fs::write(&helpers_path, "helper <- function() 1\n").unwrap();

        let script_path = script_dir.path().join("script.R");
        std::fs::write(&script_path, "source(\"helpers.R\")\nhelper\n").unwrap();
        let script_uri = lsp_types::Url::from_file_path(&script_path).unwrap();

        let script_doc = Document::new("source(\"helpers.R\")\nhelper\n", None);

        // Neither file is in state.documents
        let state = WorldState::default();

        let params = make_params(script_uri, 1, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 6),
                    }
                );
            }
        );
    }

    #[test]
    fn test_script_source_transitive() {
        // script.R sources a.R, a.R sources b.R.
        // script.R should see b.R's exports transitively.
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("b.R"),
            "library(dplyr)\nfrom_b <- function() 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a.R"),
            "source(\"b.R\")\nfrom_a <- function() 2\n",
        )
        .unwrap();

        let script_doc = Document::new("source(\"a.R\")\nfrom_a\nfrom_b\nmutate\n", None);
        let script_uri = lsp_types::Url::from_file_path(dir.path().join("script.R")).unwrap();

        let Some(library) = r_library() else {
            eprintln!("skipping: R not found");
            return;
        };

        let mut state = make_state(&script_uri, &script_doc);
        state.library = library;

        // `from_a` (line 1) — defined in a.R
        let a_uri = lsp_types::Url::from_file_path(dir.path().join("a.R")).unwrap();
        let params = make_params(script_uri.clone(), 1, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, a_uri);
            }
        );

        // `from_b` (line 2) — defined in b.R, reachable transitively
        let b_uri = lsp_types::Url::from_file_path(dir.path().join("b.R")).unwrap();
        let params = make_params(script_uri.clone(), 2, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, b_uri);
            }
        );

        // `mutate` (line 3) — from dplyr, attached by b.R's library() call.
        // Package symbol, no NavigationTarget.
        let params = make_params(script_uri, 3, 0);
        let result = goto_definition(&script_doc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_script_source_cycle_does_not_hang() {
        // a.R sources b.R, b.R sources a.R. Should not recurse infinitely.
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("a.R"), "source(\"b.R\")\nfrom_a <- 1\n").unwrap();
        std::fs::write(dir.path().join("b.R"), "source(\"a.R\")\nfrom_b <- 2\n").unwrap();

        let script_doc = Document::new("source(\"a.R\")\nfrom_a\nfrom_b\n", None);
        let script_uri = lsp_types::Url::from_file_path(dir.path().join("script.R")).unwrap();

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        // Should resolve without hanging. Both symbols are reachable
        // because a.R is visited first (gets its exports + b.R's exports),
        // and b.R's attempt to re-source a.R is a no-op due to cycle detection.
        let a_uri = lsp_types::Url::from_file_path(dir.path().join("a.R")).unwrap();
        let params = make_params(script_uri.clone(), 1, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, a_uri);
            }
        );

        let b_uri = lsp_types::Url::from_file_path(dir.path().join("b.R")).unwrap();
        let params = make_params(script_uri, 2, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, b_uri);
            }
        );
    }

    #[test]
    fn test_script_source_in_function_scoping() {
        // source() inside a function body: definitions are visible inside
        // the function and its nested scopes, but NOT outside.
        // This is independent of `local = TRUE/FALSE`. Even if sourced in the
        // global environment, only code following the `source()` call in the
        // current scope can assume these symbols are in scope.
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("helpers.R"), "helper <- function() 1\n").unwrap();

        //  Line 0: "f <- function() {\n"
        //  Line 1: "  source(\"helpers.R\")\n"
        //  Line 2: "  helper\n"               <- inside f, after source()
        //  Line 3: "  function() helper\n"    <- nested scope inside f
        //  Line 4: "}\n"
        //  Line 5: "helper\n"                 <- outside f
        let script_source =
            "f <- function() {\n  source(\"helpers.R\")\n  helper\n  function() helper\n}\nhelper\n";
        let script_doc = Document::new(script_source, None);
        let script_uri = lsp_types::Url::from_file_path(dir.path().join("script.R")).unwrap();

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        let helpers_uri = lsp_types::Url::from_file_path(dir.path().join("helpers.R")).unwrap();

        // `helper` on line 2 (inside f, after source()) — should resolve
        let params = make_params(script_uri.clone(), 2, 2);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, helpers_uri);
            }
        );

        // `helper` on line 3 (nested function inside f) — should resolve
        let params = make_params(script_uri.clone(), 3, 14);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, helpers_uri);
            }
        );

        // `helper` on line 5 (outside f) — should NOT resolve
        let params = make_params(script_uri, 5, 0);
        let result = goto_definition(&script_doc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_script_source_does_not_shadow_local_def() {
        // A local definition should take precedence over a sourced one.
        // `source()` at file scope defines `foo`, but a subsequent local
        // `foo <- "local"` should shadow it.
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("helpers.R"), "foo <- function() 1\n").unwrap();

        //  Line 0: "source(\"helpers.R\")\n"
        //  Line 1: "foo <- \"local\"\n"
        //  Line 2: "foo\n"
        let script_source = "source(\"helpers.R\")\nfoo <- \"local\"\nfoo\n";
        let script_doc = Document::new(script_source, None);
        let script_uri = lsp_types::Url::from_file_path(dir.path().join("script.R")).unwrap();

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        // `foo` on line 2 should resolve to the LOCAL definition on line 1,
        // not to the sourced one from helpers.R.
        let params = make_params(script_uri.clone(), 2, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, script_uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(1, 0),
                        end: lsp_types::Position::new(1, 3),
                    }
                );
            }
        );
    }

    #[test]
    fn test_script_source_diamond_dependency() {
        // Diamond: a.R and b.R both source helpers.R.
        // script.R sources both a.R and b.R.
        // `helper` (from helpers.R) should be visible — the grey-set
        // cycle detection must not prevent re-resolving a shared dependency.
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("helpers.R"), "helper <- function() 1\n").unwrap();
        std::fs::write(
            dir.path().join("a.R"),
            "source(\"helpers.R\")\nfrom_a <- 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.R"),
            "source(\"helpers.R\")\nfrom_b <- 2\n",
        )
        .unwrap();

        let script_doc = Document::new(
            "source(\"a.R\")\nsource(\"b.R\")\nhelper\nfrom_a\nfrom_b\n",
            None,
        );
        let script_uri = lsp_types::Url::from_file_path(dir.path().join("script.R")).unwrap();

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        let helpers_uri = lsp_types::Url::from_file_path(dir.path().join("helpers.R")).unwrap();
        let a_uri = lsp_types::Url::from_file_path(dir.path().join("a.R")).unwrap();
        let b_uri = lsp_types::Url::from_file_path(dir.path().join("b.R")).unwrap();

        // `helper` (line 2) — from helpers.R, reachable through both a.R and b.R
        let params = make_params(script_uri.clone(), 2, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, helpers_uri);
            }
        );

        // `from_a` (line 3) — from a.R
        let params = make_params(script_uri.clone(), 3, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, a_uri);
            }
        );

        // `from_b` (line 4) — from b.R
        let params = make_params(script_uri, 4, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, b_uri);
            }
        );
    }

    #[test]
    fn test_script_source_self_reference() {
        // script.R sources itself. The grey-set pre-seeds the current file
        // so the self-reference is a no-op and doesn't create duplicate
        // definitions.
        let dir = tempfile::tempdir().unwrap();

        let script_path = dir.path().join("script.R");
        std::fs::write(&script_path, "source(\"script.R\")\nmy_var <- 1\nmy_var\n").unwrap();
        let script_uri = lsp_types::Url::from_file_path(&script_path).unwrap();

        let script_doc = Document::new("source(\"script.R\")\nmy_var <- 1\nmy_var\n", None);

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        // `my_var` (line 2) should resolve to its own definition on line 1,
        // not to a Sourced duplicate.
        let params = make_params(script_uri.clone(), 2, 0);
        assert_matches!(
            goto_definition(&script_doc, params, &state).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, script_uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(1, 0),
                        end: lsp_types::Position::new(1, 6),
                    }
                );
            }
        );
    }

    #[test]
    fn test_script_source_in_function_packages_scoped() {
        // source() inside a function where the sourced file calls library(dplyr).
        // The dplyr Attach directive should be scoped to the function: visible
        // inside f (and nested scopes) but NOT at file scope.
        //
        // We verify by checking the scope chain at two offsets. Package symbols
        // produce no NavigationTarget, so goto_definition can't distinguish
        // "resolved to package" from "unresolved". The scope chain check is
        // more direct.
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("helpers.R"),
            "library(dplyr)\nhelper <- function() 1\n",
        )
        .unwrap();

        //  "f <- function() {\n"              offset 0
        //  "  source(\"helpers.R\")\n"         offset 18
        //  "  mutate\n"                        offset 39
        //  "}\n"                               offset 48
        //  "mutate\n"                          offset 50
        let script_source = "f <- function() {\n  source(\"helpers.R\")\n  mutate\n}\nmutate\n";
        let script_doc = Document::new(script_source, None);
        let script_uri = lsp_types::Url::from_file_path(dir.path().join("script.R")).unwrap();

        let mut state = WorldState::default();
        state
            .documents
            .insert(script_uri.clone(), script_doc.clone());

        let (index, file_scope) = state.file_analysis(&script_uri, &script_doc);

        let has_dplyr = |layers: &[oak_index::external::BindingSource]| -> bool {
            layers.iter().any(|l| matches!(l, oak_index::external::BindingSource::PackageExports(pkg) if pkg == "dplyr"))
        };

        // Inside f (offset 41, on "mutate"): dplyr should be in the scope chain
        let inner_offset = biome_rowan::TextSize::from(41);
        let inner_chain = file_scope.at(&index, inner_offset);
        assert!(has_dplyr(&inner_chain));

        // Outside f (offset 50, on "mutate"): dplyr should NOT be in the scope chain
        let outer_offset = biome_rowan::TextSize::from(50);
        let outer_chain = file_scope.at(&index, outer_offset);
        assert!(!has_dplyr(&outer_chain));
    }
}
