use std::path::Path;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_path::FilePath;
use anyhow::anyhow;
use stdext::result::ResultExt;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::ReferenceParams;
use tower_lsp::lsp_types::Url;
use tree_sitter::Node;
use tree_sitter::Point;
use walkdir::WalkDir;

use crate::lsp;
use crate::lsp::document::Document;
use crate::lsp::indexer::filter_entry;
use crate::lsp::state::with_document;
use crate::lsp::state::WorldState;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::url::UrlExt;
use crate::treesitter::ExtractOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(crate) fn find_references(
    params: ReferenceParams,
    state: &WorldState,
) -> anyhow::Result<Vec<Location>> {
    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let include_declaration = params.context.include_declaration;

    let document = state.get_document(&FilePath::from_url(&uri))?;

    let mut locations: Vec<Location> = Vec::new();

    let index = document.semantic_index();
    let root = document.syntax()?;

    // Intra-file resolution is precise via the semantic index
    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;
    let pos = oak_ide::FilePosition {
        file: uri.clone(),
        offset,
    };
    let intra = oak_ide::find_references(&index, &root, &pos, include_declaration);
    let intra_resolved = !intra.ranges.is_empty();
    let target_locally_scoped = intra.locally_scoped;

    for file_range in intra.ranges {
        let Some(range) = to_proto::range(
            file_range.range,
            &document.line_index,
            document.position_encoding,
        )
        .log_err() else {
            continue;
        };
        locations.push(Location::new(file_range.file, range));
    }

    // Skip the cross-file textual walk when the target is function-scoped.
    // Local bindings aren't visible to other files, so any same-name match
    // elsewhere is by definition a different binding. TODO(salsa): the
    // short-circuit moves inside `oak_ide::find_references` once cross-file
    // resolution lands; the `locally_scoped` flag goes away.
    if target_locally_scoped {
        return Ok(locations);
    }

    // Run the textual walk to pick up cross-file references. When intra-file
    // resolved cleanly, skip the current file (intra-file is authoritative
    // there). When it didn't, include the current file so an unbound symbol
    // still surfaces its own occurrences. Cross-file results are textual
    // candidates only, so until proper imports resolution lands they may
    // include false positives (other bindings that happen to share the name).
    let skip_current = if intra_resolved {
        uri.to_file_path().ok()
    } else {
        None
    };
    if let Ok(context) = build_context(&uri, position, state) {
        for folder in state.workspace.folders.iter() {
            if let Ok(path) = folder.to_file_path() {
                lsp::log_info!("searching references in folder {}", path.display());
                find_references_in_folder(
                    &context,
                    &path,
                    skip_current.as_deref(),
                    &mut locations,
                    state,
                );
            }
        }
    }

    Ok(locations)
}

#[derive(Debug, PartialEq)]
enum ReferenceKind {
    Symbol, // a regular R symbol
    Dollar, // a dollar name, following '$'
    At,     // a slot name, following '@'
}

// Assuming `x` is an `identifier`, is it the RHS of a `$` or `@`?
fn node_reference_kind(x: &Node) -> ReferenceKind {
    let Some(parent) = x.parent() else {
        // No `parent`, must be a regular symbol
        return ReferenceKind::Symbol;
    };

    let parent_type = parent.node_type();

    if !matches!(parent_type, NodeType::ExtractOperator(_)) {
        // Parent not `$` or `@`
        return ReferenceKind::Symbol;
    }

    // Need to check that we actually came from the RHS
    let Some(rhs) = parent.child_by_field_name("rhs") else {
        return ReferenceKind::Symbol;
    };
    if &rhs != x {
        return ReferenceKind::Symbol;
    };

    match parent_type {
        NodeType::ExtractOperator(ExtractOperatorType::Dollar) => ReferenceKind::Dollar,
        NodeType::ExtractOperator(ExtractOperatorType::At) => ReferenceKind::At,
        _ => std::unreachable!(),
    }
}

struct Context {
    kind: ReferenceKind,
    symbol: String,
}

fn add_reference(
    node: &Node,
    document: &Document,
    path: &Path,
    locations: &mut Vec<Location>,
) -> anyhow::Result<()> {
    let start = document.lsp_position_from_tree_sitter_point(node.start_position())?;
    let end = document.lsp_position_from_tree_sitter_point(node.end_position())?;

    let location = Location::new(
        Url::from_file_path(path).expect("valid path"),
        Range::new(start, end),
    );
    locations.push(location);
    Ok(())
}

fn found_match(node: &Node, contents: &str, context: &Context) -> bool {
    if !node.is_identifier() {
        return false;
    }
    let Ok(symbol) = node.node_to_string(contents) else {
        return false;
    };
    if symbol != context.symbol {
        return false;
    }
    context.kind == node_reference_kind(node)
}

fn build_context(uri: &Url, position: Position, state: &WorldState) -> anyhow::Result<Context> {
    let path = uri.file_path()?;

    with_document(path.as_path(), state, |document| {
        let ast = &document.ast;
        let contents = document.contents.as_str();
        let point = document.tree_sitter_point_from_lsp_position(position)?;

        let mut node = ast
            .root_node()
            .descendant_for_point_range(point, point)
            .into_result()?;

        // Zero-width range queries at an identifier boundary return the
        // wrapping node rather than the identifier itself. If the cursor is at
        // the trailing edge of a selection (column past the last character),
        // retry one column back. If it's at the leading edge (column on the
        // first character), retry one column forward.
        if !node.is_identifier() && point.column > 0 {
            let back = Point::new(point.row, point.column - 1);
            if let Some(retry) = ast
                .root_node()
                .descendant_for_point_range(back, back)
                .filter(|n| n.is_identifier())
            {
                node = retry;
            }
        }
        if !node.is_identifier() {
            let fwd = Point::new(point.row, point.column + 1);
            if let Some(retry) = ast
                .root_node()
                .descendant_for_point_range(fwd, fwd)
                .filter(|n| n.is_identifier())
            {
                node = retry;
            }
        }

        if !node.is_identifier() {
            return Err(anyhow!(
                "couldn't find an identifier associated with point {point:?}",
            ));
        }

        let kind = node_reference_kind(&node);
        let symbol = node.node_to_string(contents)?;

        Ok(Context { kind, symbol })
    })
}

fn find_references_in_folder(
    context: &Context,
    path: &Path,
    skip_path: Option<&Path>,
    locations: &mut Vec<Location>,
    state: &WorldState,
) {
    let walker = WalkDir::new(path);
    for entry in walker.into_iter().filter_entry(filter_entry) {
        let entry = unwrap!(entry, Err(_) => { continue; });
        let path = entry.path();
        let ext = unwrap!(path.extension(), None => { continue; });
        if ext != "r" && ext != "R" {
            continue;
        }
        if skip_path.is_some_and(|p| p == path) {
            // Caller's intra-file pass already produced refs for this file.
            continue;
        }

        let result = with_document(path, state, |document| {
            find_references_in_document(context, path, document, locations)
        });

        match result {
            Ok(result) => result,
            Err(_error) => {
                lsp::log_warn!("error retrieving document for path {}", path.display());
                continue;
            },
        }
    }
}

fn find_references_in_document(
    context: &Context,
    path: &Path,
    document: &Document,
    locations: &mut Vec<Location>,
) -> anyhow::Result<()> {
    let ast = &document.ast;
    let contents = document.contents.as_str();

    let mut cursor = ast.walk();
    cursor.recurse(|node| {
        if found_match(&node, contents, context) {
            add_reference(&node, document, path, locations).log_err();
        }

        true
    });
    Ok(())
}
