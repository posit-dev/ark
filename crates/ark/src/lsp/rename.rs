//
// rename.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use std::collections::HashMap;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_syntax::RIdentifier;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use biome_rowan::TokenAtOffset;
use lsp_types::PrepareRenameResponse;
use lsp_types::RenameParams;
use lsp_types::TextDocumentPositionParams;
use lsp_types::TextEdit;
use lsp_types::WorkspaceEdit;
use oak_index::builder::build;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::SymbolFlags;
use oak_index::semantic_index::SymbolId;
use tower_lsp::lsp_types;

use crate::lsp::document::Document;

pub(crate) fn prepare_rename(
    document: &Document,
    params: TextDocumentPositionParams,
) -> anyhow::Result<Option<PrepareRenameResponse>> {
    let offset = from_proto::offset_from_position(
        params.position,
        &document.line_index,
        document.position_encoding,
    )?;

    let Some(ident) = find_identifier_at_offset(document, offset) else {
        return Ok(None);
    };

    let name = identifier_text(&ident);
    let index = build(&document.parse.tree());
    let scope = index.scope_at(offset);

    if index.resolve_symbol(&name, scope).is_none() {
        return Ok(None);
    }

    let lsp_range = to_proto::range(
        ident.syntax().text_trimmed_range(),
        &document.line_index,
        document.position_encoding,
    )?;

    Ok(Some(PrepareRenameResponse::Range(lsp_range)))
}

pub(crate) fn rename(
    document: &Document,
    params: RenameParams,
) -> anyhow::Result<Option<WorkspaceEdit>> {
    let uri = params.text_document_position.text_document.uri.clone();
    let position = params.text_document_position.position;
    let new_name = params.new_name;

    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;

    let Some(ident) = find_identifier_at_offset(document, offset) else {
        return Ok(None);
    };

    let name = identifier_text(&ident);
    let index = build(&document.parse.tree());
    let scope = index.scope_at(offset);

    let Some((defining_scope, symbol_id)) = index.resolve_symbol(&name, scope) else {
        return Ok(None);
    };

    let ranges = collect_rename_ranges(&index, &name, defining_scope, symbol_id);

    let edits: Vec<TextEdit> = ranges
        .into_iter()
        .map(|range| -> anyhow::Result<TextEdit> {
            let lsp_range =
                to_proto::range(range, &document.line_index, document.position_encoding)?;
            Ok(TextEdit {
                range: lsp_range,
                new_text: new_name.clone(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let changes = HashMap::from([(uri, edits)]);

    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

fn find_identifier_at_offset(document: &Document, offset: TextSize) -> Option<RIdentifier> {
    let token = match document.syntax().token_at_offset(offset) {
        TokenAtOffset::None => return None,
        TokenAtOffset::Single(token) => token,
        TokenAtOffset::Between(left, right) => {
            if left
                .parent()
                .is_some_and(|p| RIdentifier::can_cast(p.kind()))
            {
                left
            } else {
                right
            }
        },
    };

    let parent = token.parent()?;

    if !RIdentifier::can_cast(parent.kind()) {
        return None;
    }

    RIdentifier::cast(parent)
}

fn identifier_text(ident: &RIdentifier) -> String {
    let text = ident.syntax().text_trimmed().to_string();
    match text.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
        Some(inner) => inner.to_string(),
        None => text,
    }
}

fn collect_rename_ranges(
    index: &SemanticIndex,
    name: &str,
    defining_scope: ScopeId,
    symbol_id: SymbolId,
) -> Vec<TextRange> {
    let mut ranges = Vec::new();

    // Collect all definitions and uses in the defining scope.
    // This includes super-assignment definitions that target this scope from
    // nested scopes.
    for (_, def) in index.definitions(defining_scope).iter() {
        if def.symbol() == symbol_id {
            ranges.push(def.range());
        }
    }

    for (_, use_site) in index.uses(defining_scope).iter() {
        if use_site.symbol() == symbol_id {
            ranges.push(use_site.range());
        }
    }

    // Walk descendant scopes, collecting uses for the same name while
    // skipping subtrees that shadow it with their own binding.
    let mut stack: Vec<ScopeId> = index.child_scopes(defining_scope).collect();

    while let Some(child_scope) = stack.pop() {
        let symbols = index.symbols(child_scope);

        if let Some(symbol) = symbols.get(name) {
            if symbol.flags().contains(SymbolFlags::IS_BOUND) {
                // This scope shadows the name, skip entire subtree
                continue;
            }

            if let Some(child_symbol_id) = symbols.id(name) {
                for (_, use_site) in index.uses(child_scope).iter() {
                    if use_site.symbol() == child_symbol_id {
                        ranges.push(use_site.range());
                    }
                }
            }
        }

        // Recurse into this child's children
        stack.extend(index.child_scopes(child_scope));
    }

    ranges
}

#[cfg(test)]
mod tests {
    use lsp_types::Position;
    use lsp_types::Range;
    use tower_lsp::lsp_types;

    use super::*;
    use crate::lsp::document::Document;
    use crate::lsp::util::test_path;

    /// Run rename at (line, col) and return sorted edit ranges.
    fn rename_ranges(code: &str, line: u32, col: u32) -> Option<Vec<Range>> {
        let doc = Document::new(code, None);
        let uri = test_path("test.R");

        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(line, col),
            },
            new_name: String::from("new_name"),
            work_done_progress_params: Default::default(),
        };

        let result = rename(&doc, params).unwrap()?;
        let changes = result.changes?;
        let mut edits: Vec<Range> = changes[&uri].iter().map(|e| e.range).collect();
        edits.sort_by_key(|r| (r.start.line, r.start.character));
        Some(edits)
    }

    fn r(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range {
            start: Position::new(sl, sc),
            end: Position::new(el, ec),
        }
    }

    #[test]
    fn test_rename_simple() {
        let ranges = rename_ranges("x <- 1\nprint(x)\n", 1, 6).unwrap();
        assert_eq!(ranges, vec![r(0, 0, 0, 1), r(1, 6, 1, 7)]);
    }

    #[test]
    fn test_rename_from_definition_site() {
        let ranges = rename_ranges("foo <- 1\nprint(foo)\n", 0, 0).unwrap();
        assert_eq!(ranges, vec![r(0, 0, 0, 3), r(1, 6, 1, 9)]);
    }

    #[test]
    fn test_prepare_rename_returns_range() {
        let doc = Document::new("x <- 1\nprint(x)\n", None);
        let params = TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: test_path("test.R"),
            },
            position: Position::new(1, 6),
        };
        let result = prepare_rename(&doc, params).unwrap();
        assert_eq!(result, Some(PrepareRenameResponse::Range(r(1, 6, 1, 7))));
    }

    #[test]
    fn test_prepare_rename_unresolved_symbol() {
        let doc = Document::new("print(x)\n", None);
        let params = TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: test_path("test.R"),
            },
            position: Position::new(0, 6),
        };
        assert!(prepare_rename(&doc, params).unwrap().is_none());
    }

    #[test]
    fn test_rename_parameter() {
        let ranges = rename_ranges("f <- function(x) x + 1\n", 0, 17).unwrap();
        assert_eq!(ranges, vec![r(0, 14, 0, 15), r(0, 17, 0, 18)]);
    }

    #[test]
    fn test_rename_shadowed_outer() {
        let code = "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\nx\n";
        let ranges = rename_ranges(code, 5, 0).unwrap();
        assert_eq!(ranges, vec![r(0, 0, 0, 1), r(5, 0, 5, 1)]);
    }

    #[test]
    fn test_rename_shadowed_inner() {
        let code = "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\nx\n";
        let ranges = rename_ranges(code, 3, 2).unwrap();
        assert_eq!(ranges, vec![r(2, 2, 2, 3), r(3, 2, 3, 3)]);
    }

    #[test]
    fn test_rename_non_shadowing_nested_scope() {
        let code = "x <- 1\nf <- function() x\n";
        let ranges = rename_ranges(code, 0, 0).unwrap();
        assert_eq!(ranges, vec![r(0, 0, 0, 1), r(1, 16, 1, 17)]);
    }

    #[test]
    fn test_rename_super_assignment() {
        let code = "x <- 1\nf <- function() {\n  x <<- 2\n}\nx\n";
        let ranges = rename_ranges(code, 0, 0).unwrap();
        assert_eq!(ranges, vec![r(0, 0, 0, 1), r(2, 2, 2, 3), r(4, 0, 4, 1)]);
    }

    #[test]
    fn test_rename_for_variable() {
        let ranges = rename_ranges("for (i in 1:10) print(i)\n", 0, 5).unwrap();
        assert_eq!(ranges, vec![r(0, 5, 0, 6), r(0, 22, 0, 23)]);
    }

    #[test]
    fn test_rename_cursor_on_non_identifier() {
        assert!(rename_ranges("1 + 2\n", 0, 2).is_none());
    }

    #[test]
    fn test_rename_multiple_definitions() {
        let code = "x <- 1\nx <- 2\nprint(x)\n";
        let ranges = rename_ranges(code, 2, 6).unwrap();
        assert_eq!(ranges, vec![r(0, 0, 0, 1), r(1, 0, 1, 1), r(2, 6, 2, 7)]);
    }

    #[test]
    fn test_rename_edits_have_new_name() {
        let doc = Document::new("x <- 1\nprint(x)\n", None);
        let uri = test_path("test.R");
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(1, 6),
            },
            new_name: String::from("bar"),
            work_done_progress_params: Default::default(),
        };
        let result = rename(&doc, params).unwrap().unwrap();
        let edits = &result.changes.unwrap()[&uri];
        assert!(edits.iter().all(|e| e.new_text == "bar"));
    }
}
