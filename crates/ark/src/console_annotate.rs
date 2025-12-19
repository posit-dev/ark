//
// console_annotate.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use aether_syntax::RBracedExpressions;
use aether_syntax::RExpressionList;
use aether_syntax::RLanguage;
use aether_syntax::RRoot;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use amalthea::wire::execute_request::CodeLocation;
use anyhow::anyhow;
use biome_line_index::LineIndex;
use biome_rowan::syntax::SyntaxElementKey;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::SyntaxRewriter;
use biome_rowan::TriviaPieceKind;
use biome_rowan::VisitNodeSignal;
use biome_rowan::WalkEvent;
use harp::object::RObject;
use libr::SEXP;
use url::Url;

use crate::dap::dap::Breakpoint;
use crate::dap::dap::BreakpointState;
use crate::interface::RMain;

/// Function name used for auto-stepping over injected calls such as breakpoints
const AUTO_STEP_FUNCTION: &str = ".ark_auto_step";

// Main responsibilities of code annotation:
//
// 1. Inject breakpoints in the code, along with line directives that map to
//    original document lines. Breakpoints can only be injected in the top-level
//    expression list or in a `{}` list.
//
// 2. Mark invalid breakpoints as such in the DAP state. This concerns
//    breakpoints on a closing `}` or inside multi-line expressions.
//    The breakpoints are marked invalid by mutation as soon as they are matched to
//    an invalid location by our code walker.
//
// 3. When sourcing with `source()` or `load_all()`, wrap in `{}` to allow
//    top-level stepping, inject breakpoints, and inject top-level verification
//    calls to let Ark know a breakpoint is now active after evaluation. This is
//    handled in a separate code path in `annotate_source()`.
//
// Breakpoint injection happens in two phases:
//
// - We first collect "anchors", i.e. the syntax node where a breakpoint should
//   be injected, its line in document coordinates, and a unique identifier.
//
// - In a second pass we use a Biome `SyntaxRewriter` to go ahead and modify the
//   code. This is a tree visitor that allows replacing the node on the way out
//   (currently via an extension to the Biome API that will be submitted to Biome
//   later on). This approach was chosen over `BatchMutation`, which collects
//   changes and applies them from deepest to shallowest, because the latter:
//
//   - Doesn't handle insertions in lists (though that could be contributed).
//     Only replacements are currently supported.
//
//   - Doesn't handle nested changes in a node that is later _replaced_.
//     If the scheduled changes were pure insertions (if insertions were
//     supported) then nested changes would compose correctly. However, nested
//     changes wouldn't work well as soon as a replacement is involved, because
//     BatchMutation can't express "edit a descendant" and "replace an ancestor"
//     in one batch without risking the ancestor replacement overwriting the
//     descendant edit.
//
//     That limitation interacts badly with Biome's strict stance on mutation.
//     For example, you can't add a comment to a node; you have to create a new one
//     that features a comment. This issue arises when adding a line directive, e.g. in:
//
//     ```r
//     {     # BP 1
//        1  # BP 2
//     }
//     ```
//
//     BP 2 causes changes inside the braces. Then BP 1 causes the whole brace
//     expression to be replaced with a variant that has a line directive attached.
//     But there is no way to express both these changes to BatchMutation because it
//     takes modifications upfront. This is why we work instead with `SyntaxRewriter`
//     which allows us to replace nodes from bottom to top as we go.
//
//     Note that Rust-Analyzer's version of Rowan is much more flexible and allow you to
//     create a mutable syntax tree that you can freely update (see `clone_for_update()`
//     and the tree editor API). Unfortunately Biome has adopted a strict stance on
//     immutable data structures so we don't have access to such affordances.

// Called by ReadConsole to inject breakpoints (if any) and source reference
// mapping (via a line directive)
pub(crate) fn annotate_input(
    code: &str,
    location: CodeLocation,
    breakpoints: Option<&mut [Breakpoint]>,
) -> anyhow::Result<String> {
    // First, inject breakpoints into the original code. This must happen before
    // we add the outer line directive, otherwise the coordinates of inner line
    // directives are shifted by 1 line.
    let code_with_breakpoints = if let Some(breakpoints) = breakpoints {
        let line_index = LineIndex::new(code);
        inject_breakpoints(code, location.clone(), breakpoints, &line_index)?
    } else {
        code.to_string()
    };

    // Now add the line directive to the (possibly modified) code. This maps the
    // code to evaluate to a location in the original document.
    let line_directive = format!(
        "#line {line} \"{uri}\"",
        line = location.start.line + 1,
        uri = location.uri
    );

    // Add leading whitespace to ensure that R starts parsing expressions from
    // the expected `character` offset
    let leading_padding = " ".repeat(location.start.character as usize);

    Ok(format!(
        "{line_directive}\n{leading_padding}{code_with_breakpoints}"
    ))
}

pub(crate) fn inject_breakpoints(
    code: &str,
    location: CodeLocation,
    breakpoints: &mut [Breakpoint],
    line_index: &LineIndex,
) -> anyhow::Result<String> {
    let root = aether_parser::parse(code, Default::default()).tree();

    // The offset between document coordinates and code coordinates. Breakpoints
    // are in document coordinates, but AST nodes are in code coordinates
    // (starting at line 0).
    let line_offset = location.start.line;

    // Filter breakpoints to only those within the source's valid range. We
    // collect both for simplicity and because we need to sort the vector
    // later on.
    let breakpoints: Vec<_> = breakpoints
        .iter_mut()
        .filter(|bp| bp.line >= location.start.line && bp.line <= location.end.line)
        .collect();

    if breakpoints.is_empty() {
        return Ok(code.to_string());
    }

    // First collect all breakpoint anchors, then inject in a separate pass.
    // This two-stage approach is not necessary but keeps the anchor-finding
    // logic (with its edge cases like invalid breakpoints, nesting decisions,
    // look-ahead) separate from the tree transformation with the
    // `SyntaxRewriter`.
    let anchors = find_breakpoint_anchors(root.syntax(), breakpoints, line_index, line_offset)?;

    if anchors.is_empty() {
        return Ok(code.to_string());
    }

    // Build map of anchor key -> (breakpoint_id, doc_line).
    // Anchors already store document coordinates.
    let breakpoint_map: std::collections::HashMap<_, _> = anchors
        .into_iter()
        .map(|a| (a.anchor.key(), (a.breakpoint_id, a.doc_line)))
        .collect();

    // Now inject breakpoints with a `SyntaxRewriter`. This is the most
    // practical option we have with Biome's Rowan because `BatchMutation` does
    // not support 1-to-2 splicing (insert breakpoint call before an expression,
    // keeping the original).
    let mut rewriter = BreakpointRewriter::new(&location.uri, breakpoint_map);
    let transformed = rewriter.transform(root.syntax().clone());

    if let Some(err) = rewriter.take_err() {
        return Err(err);
    }

    Ok(transformed.to_string())
}

/// Annotate source code for `source()` and `pkgload::load_all()`.
///
/// - Wraps the whole source in a `{}` block. This allows R to step through the
///   top-level expressions.
/// - Injects breakpoint calls (`.ark_auto_step(.ark_breakpoint(...))`) at
///   breakpoint locations.
/// - Injects verification calls (`.ark_auto_step(.ark_verify_breakpoints_range(...))`)
///   after each top-level expression. Verifying expression by expression allows
///   marking breakpoints as verified even when an expression fails mid-script.
/// - `#line` directives before each original expression so the debugger knows
///   where to step in the original file.
pub(crate) fn annotate_source(
    code: &str,
    uri: &Url,
    breakpoints: &mut [Breakpoint],
) -> anyhow::Result<String> {
    let line_index = LineIndex::new(code);

    let root = aether_parser::parse(code, Default::default()).tree();
    let root_node = RRoot::cast(root.syntax().clone())
        .ok_or_else(|| anyhow!("Failed to cast parsed tree to RRoot"))?;

    // Collect line ranges for top-level expressions BEFORE any modifications
    let top_level_ranges: Vec<std::ops::Range<u32>> = root_node
        .expressions()
        .into_iter()
        .map(|expr| text_trimmed_line_range(expr.syntax(), &line_index))
        .collect::<anyhow::Result<Vec<_>>>()?;

    if top_level_ranges.is_empty() {
        return Ok(code.to_string());
    }

    // Find breakpoint anchors (may be nested within top-level expressions)
    let bp_vec: Vec<_> = breakpoints.iter_mut().collect();
    let anchors = find_breakpoint_anchors(root.syntax(), bp_vec, &line_index, 0)?;

    // Build map of anchor key -> (breakpoint_id, doc_line).
    let breakpoint_map: std::collections::HashMap<_, _> = anchors
        .into_iter()
        .map(|a| (a.anchor.key(), (a.breakpoint_id, a.doc_line)))
        .collect();

    let mut rewriter = BreakpointRewriter::new(uri, breakpoint_map);
    let transformed = rewriter.transform(root.syntax().clone());

    if let Some(err) = rewriter.take_err() {
        return Err(err);
    }

    let transformed_root = RRoot::cast(transformed)
        .ok_or_else(|| anyhow!("Failed to cast transformed tree to RRoot"))?;

    // Rebuild root expression list with #line directives and verify calls
    let annotated = annotate_root_list(
        transformed_root.expressions().syntax().clone(),
        &top_level_ranges,
        uri,
    )?;

    // Wrap in braces so R can step through expressions
    Ok(format!("{{\n{annotated}}}\n"))
}

struct BreakpointAnchor {
    /// Unique identifier for the breakpoint, injected as argument in the code
    breakpoint_id: i64,
    /// The line in document coordinates (0-based)
    doc_line: u32,
    /// The anchor node (expression to place breakpoint before)
    anchor: RSyntaxNode,
}

fn find_breakpoint_anchors(
    root: &RSyntaxNode,
    mut breakpoints: Vec<&mut Breakpoint>,
    line_index: &LineIndex,
    line_offset: u32,
) -> anyhow::Result<Vec<BreakpointAnchor>> {
    // Sort breakpoints by ascending line so we can walk the expression lists in
    // DFS order, and match breakpoints to expressions by comparing lines. Both
    // sequences proceed in roughly the same order (by line number), so we can
    // consume breakpoints one by one as we find their anchors without needing
    // to go backward in either sequence.
    breakpoints.sort_by_key(|bp| bp.line);

    // Peekable so we can inspect the next breakpoint's line without consuming it,
    // deciding whether to place it at the current expression or continue to the
    // next expression without consuming the current breakpoint.
    let mut bp_iter = breakpoints.into_iter().peekable();

    let mut anchors = Vec::new();

    // Start from the root's expression list
    let r =
        RRoot::cast(root.clone()).ok_or_else(|| anyhow!("Failed to cast parsed tree to RRoot"))?;
    let root_list = r.expressions();

    find_anchors_in_list(
        &root_list,
        &mut bp_iter,
        &mut anchors,
        line_index,
        line_offset,
        true,
    )?;

    Ok(anchors)
}

// Takes an expression list, either from the root node or a brace node
fn find_anchors_in_list<'a>(
    list: &RExpressionList,
    breakpoints: &mut std::iter::Peekable<impl Iterator<Item = &'a mut Breakpoint>>,
    anchors: &mut Vec<BreakpointAnchor>,
    line_index: &LineIndex,
    line_offset: u32,
    is_root: bool,
) -> anyhow::Result<()> {
    // Collect to allow indexed look-ahead and re-checking the same element
    // without consuming an iterator
    let elements: Vec<_> = list.into_iter().collect();

    if elements.is_empty() {
        return Ok(());
    }

    let mut i = 0;
    while i < elements.len() {
        let Some(bp) = breakpoints.peek() else {
            // No more breakpoints
            return Ok(());
        };

        // Convert breakpoint line from document coordinates to code coordinates
        let bp_code_line = bp.line - line_offset;

        let current = &elements[i];
        let current_line = text_trimmed_line_range(current.syntax(), line_index)?.start;

        let next_line = if i + 1 < elements.len() {
            let next = &elements[i + 1];
            let next_line = text_trimmed_line_range(next.syntax(), line_index)?.start;

            // If the breakpoint is at or past the next element, move on
            if bp_code_line >= next_line {
                i += 1;
                continue;
            }

            // Otherwise the breakpoint is either at `current_line` or between
            // `current_line` and `next_line`
            Some(next_line)
        } else {
            // There is no next element. The breakpoint either belongs to the
            // current element or is past the current list and we need to
            // backtrack and explore sibling trees.
            None
        };

        // Try to place in a nested brace list first
        let found_nested = find_anchors_in_nested_list(
            current.syntax(),
            breakpoints,
            anchors,
            line_index,
            line_offset,
        )?;

        if found_nested {
            let Some(bp) = breakpoints.peek() else {
                // No breakpoints left to process
                return Ok(());
            };

            let bp_code_line = bp.line - line_offset;

            // If next breakpoint is at or past next element, advance
            if next_line.is_some_and(|next| bp_code_line >= next) {
                i += 1;
                continue;
            }

            // Breakpoint is still within this element but wasn't placed.
            // It means it's on a closing brace so consume it and mark invalid.
            let bp = breakpoints.next().unwrap();
            bp.state = BreakpointState::Invalid;

            i += 1;
            continue;
        }

        if is_root {
            // We never place breakpoints at top-level. R can only step through a `{` list.
            let bp = breakpoints.next().unwrap();
            bp.state = BreakpointState::Invalid;

            i += 1;
            continue;
        }

        if next_line.is_none() && bp_code_line > current_line {
            // Breakpoint is past this scope entirely, in a sibling tree. Let
            // parent handle it.
            return Ok(());
        }

        // Place breakpoint at current element of the `{` list
        let bp = breakpoints.next().unwrap();
        let doc_line = current_line + line_offset;
        bp.line = doc_line;
        anchors.push(BreakpointAnchor {
            breakpoint_id: bp.id,
            doc_line,
            anchor: current.syntax().clone(),
        });
    }

    Ok(())
}

fn find_anchors_in_nested_list<'a>(
    element: &RSyntaxNode,
    breakpoints: &mut std::iter::Peekable<impl Iterator<Item = &'a mut Breakpoint>>,
    anchors: &mut Vec<BreakpointAnchor>,
    line_index: &LineIndex,
    line_offset: u32,
) -> anyhow::Result<bool> {
    let mut found_any = false;
    let mut skip_until: Option<RSyntaxNode> = None;

    // Search for brace lists in descendants
    for event in element.preorder() {
        match event {
            WalkEvent::Leave(node) => {
                // If we're leaving the node we're skipping, clear the skip flag
                if skip_until.as_ref() == Some(&node) {
                    skip_until = None;
                }
                continue;
            },

            WalkEvent::Enter(node) => {
                // If we're currently skipping a subtree, continue
                if skip_until.is_some() {
                    continue;
                }

                if let Some(braced) = RBracedExpressions::cast(node.clone()) {
                    let expr_list = braced.expressions();
                    if !expr_list.is_empty() {
                        // Found a non-empty brace list, recurse into it
                        find_anchors_in_list(
                            &expr_list,
                            breakpoints,
                            anchors,
                            line_index,
                            line_offset,
                            false,
                        )?;
                        found_any = true;

                        // Skip this node's subtree to avoid double-processing
                        skip_until = Some(node);
                    }
                }
            },
        }
    }

    Ok(found_any)
}

/// Rewriter that injects breakpoint calls into expression lists.
///
/// We use `SyntaxRewriter` rather than `BatchMutation` because we need 1-to-2
/// splicing (insert breakpoint call before an expression, keeping the
/// original). `BatchMutation` only supports 1-to-1 or 1-to-0 replacements.
struct BreakpointRewriter<'a> {
    uri: &'a Url,

    /// Map from anchor key to (breakpoint_id, line_in_document_coords)
    breakpoint_map: std::collections::HashMap<SyntaxElementKey, (i64, u32)>,

    /// Stack of pending injections, one frame per expression list we're inside.
    injection_stack: Vec<Vec<PendingInjection>>,

    /// First error encountered during transformation (if any)
    err: Option<anyhow::Error>,
}

/// Pending injection to be applied when visiting the parent expression list.
struct PendingInjection {
    /// Slot index in the parent list
    slot_index: usize,
    /// Nodes to insert before this slot
    insert_before: Vec<RSyntaxNode>,
}

impl<'a> BreakpointRewriter<'a> {
    fn new(
        uri: &'a Url,
        breakpoint_map: std::collections::HashMap<SyntaxElementKey, (i64, u32)>,
    ) -> Self {
        Self {
            uri,
            breakpoint_map,
            injection_stack: Vec::new(),
            err: None,
        }
    }

    /// Take the error (if any) out of the rewriter.
    fn take_err(&mut self) -> Option<anyhow::Error> {
        self.err.take()
    }

    /// Record an error and return the original node unchanged.
    fn fail(&mut self, err: anyhow::Error, node: RSyntaxNode) -> RSyntaxNode {
        if self.err.is_none() {
            self.err = Some(err);
        }
        node
    }
}

impl SyntaxRewriter for BreakpointRewriter<'_> {
    type Language = RLanguage;

    fn visit_node(&mut self, node: RSyntaxNode) -> VisitNodeSignal<Self::Language> {
        // Only push frames for expression lists, not other list types
        if node.kind() == RSyntaxKind::R_EXPRESSION_LIST {
            self.injection_stack.push(Vec::new());
        }

        VisitNodeSignal::Traverse(node)
    }

    fn visit_node_post(&mut self, node: RSyntaxNode) -> RSyntaxNode {
        // If we already have an error, skip processing
        if self.err.is_some() {
            return node;
        }

        // If an expression list, apply any pending injections
        if node.kind() == RSyntaxKind::R_EXPRESSION_LIST {
            let injections = self.injection_stack.pop().unwrap_or_default();

            if injections.is_empty() {
                return node;
            } else {
                return Self::apply_injections(node, injections);
            }
        }

        let Some(&(breakpoint_id, line)) = self.breakpoint_map.get(&node.key()) else {
            // Not a breakpoint anchor, nothing to inject
            return node;
        };

        // Anchors are always inside expression lists, so we must have a frame
        let Some(frame) = self.injection_stack.last_mut() else {
            return self.fail(
                anyhow!("Breakpoint anchor found outside expression list"),
                node,
            );
        };

        // Add line directive to current node right away
        let decorated_node = match add_line_directive_to_node(&node, line, self.uri) {
            Ok(n) => n,
            Err(err) => return self.fail(err, node),
        };

        // Queue breakpoint injection for parent expression list
        let breakpoint_call = create_breakpoint_call(self.uri, breakpoint_id);
        frame.push(PendingInjection {
            slot_index: node.index(),
            insert_before: vec![breakpoint_call],
        });

        decorated_node
    }
}

impl BreakpointRewriter<'_> {
    /// Apply pending injections to an expression list node.
    fn apply_injections(
        mut node: RSyntaxNode,
        mut injections: Vec<PendingInjection>,
    ) -> RSyntaxNode {
        // Sort by slot index descending so we can splice without invalidating indices
        injections.sort_by(|a, b| b.slot_index.cmp(&a.slot_index));

        for injection in injections {
            // Insert before (at the slot index)
            if !injection.insert_before.is_empty() {
                node = node.splice_slots(
                    injection.slot_index..injection.slot_index,
                    injection.insert_before.into_iter().map(|n| Some(n.into())),
                );
            }
        }

        node
    }
}

/// Returns the line range [start, end) for the node's trimmed text.
fn text_trimmed_line_range(
    node: &RSyntaxNode,
    line_index: &LineIndex,
) -> anyhow::Result<std::ops::Range<u32>> {
    // This gives a range in offset coordinates. We need to retrieve the line range
    // using the line index.
    let text_range = node.text_trimmed_range();

    let start = line_index
        .line_col(text_range.start())
        .map(|lc| lc.line)
        .ok_or_else(|| anyhow!("Failed to get line for text range start offset"))?;

    let end = line_index
        .line_col(text_range.end())
        .map(|lc| lc.line + 1) // Close the range end
        .ok_or_else(|| anyhow!("Failed to get line for text range end offset"))?;

    Ok(start..end)
}

type TriviaPieces = Vec<(TriviaPieceKind, String)>;

/// Collects leading trivia from a token as (kind, text) tuples.
fn collect_leading_trivia(token: &aether_syntax::RSyntaxToken) -> TriviaPieces {
    token
        .leading_trivia()
        .pieces()
        .map(|piece| (piece.kind(), piece.text().to_string()))
        .collect()
}

/// Creates trivia pieces for a line directive comment followed by a newline.
fn line_directive_trivia(line: u32, uri: &Url) -> TriviaPieces {
    let directive = format!("#line {} \"{}\"", line + 1, uri);
    vec![
        (TriviaPieceKind::SingleLineComment, directive),
        (TriviaPieceKind::Newline, "\n".to_string()),
    ]
}

/// Inserts trivia pieces before trailing whitespace (indentation) if present.
/// This preserves indentation: `[\n, \n, ws]` becomes `[\n, \n, <inserted>, ws]`
fn insert_before_trailing_whitespace(
    mut trivia: TriviaPieces,
    to_insert: TriviaPieces,
) -> TriviaPieces {
    let has_trailing_whitespace = trivia
        .last()
        .is_some_and(|(k, _)| *k == TriviaPieceKind::Whitespace);

    if has_trailing_whitespace {
        let Some(last) = trivia.pop() else {
            trivia.extend(to_insert);
            return trivia;
        };
        trivia.extend(to_insert);
        trivia.push(last);
    } else {
        trivia.extend(to_insert);
    }

    trivia
}

fn add_line_directive_to_node(
    node: &RSyntaxNode,
    line: u32,
    uri: &Url,
) -> anyhow::Result<RSyntaxNode> {
    let first_token = node
        .first_token()
        .ok_or_else(|| anyhow!("Node has no first token for line directive"))?;

    let mut existing_trivia = collect_leading_trivia(&first_token);

    // Skip leading newline as it belongs to the previous node
    if existing_trivia
        .first()
        .is_some_and(|(kind, _)| *kind == TriviaPieceKind::Newline)
    {
        existing_trivia.remove(0);
    }

    let directive_trivia = line_directive_trivia(line, uri);
    let new_trivia = insert_before_trailing_whitespace(existing_trivia, directive_trivia);

    let new_first_token =
        first_token.with_leading_trivia(new_trivia.iter().map(|(k, t)| (*k, t.as_str())));

    node.clone()
        .replace_child(first_token.into(), new_first_token.into())
        .ok_or_else(|| anyhow!("Failed to replace first token with line directive"))
}

/// Rebuild the root expression list with #line directives and verify calls for
/// each expression.
fn annotate_root_list(
    list_node: RSyntaxNode,
    ranges: &[std::ops::Range<u32>],
    uri: &Url,
) -> anyhow::Result<RSyntaxNode> {
    let mut result_slots: Vec<Option<biome_rowan::SyntaxElement<RLanguage>>> = Vec::new();

    // Use pre-computed line ranges (from before any transformations)
    let mut range_iter = ranges.iter();

    for slot in list_node.slots() {
        let biome_rowan::SyntaxSlot::Node(node) = slot else {
            result_slots.push(None);
            continue;
        };

        // Get pre-computed line range for this expression
        let Some(line_range) = range_iter.next() else {
            result_slots.push(Some(node.into()));
            continue;
        };

        // Add #line directive to expression
        let decorated_node = add_line_directive_to_node(&node, line_range.start, uri)?;
        result_slots.push(Some(decorated_node.into()));

        let verify_call = create_verify_call(uri, line_range);
        result_slots.push(Some(verify_call.into()));
    }

    // Replace all slots with the new list
    let slot_count = list_node.slots().count();
    Ok(list_node.splice_slots(0..slot_count, result_slots))
}

// We create new calls by parsing strings. Although less elegant, it's much less
// verbose and easier to see what's going on.

fn create_breakpoint_call(uri: &Url, id: i64) -> RSyntaxNode {
    // NOTE: If you use `base::browser()` here in an attempt to prevent masking
    // issues in case someone redefined `browser()`, you'll cause the function
    // in which the breakpoint is injected to be bytecode-compiled. This is a
    // limitation/bug of https://github.com/r-devel/r-svn/blob/e2aae817/src/library/compiler/R/cmp.R#L1273-L1290
    let code = format!(
        "\nbase::{AUTO_STEP_FUNCTION}(base::.ark_breakpoint(browser(), \"{uri}\", \"{id}\"))\n"
    );
    aether_parser::parse(&code, Default::default()).syntax()
}

fn create_verify_call(uri: &Url, line_range: &std::ops::Range<u32>) -> RSyntaxNode {
    let code = format!(
        "\nbase::{AUTO_STEP_FUNCTION}(base::.ark_verify_breakpoints_range(\"{}\", {}L, {}L))\n",
        uri,
        line_range.start + 1,
        line_range.end + 1
    );
    aether_parser::parse(&code, Default::default()).syntax()
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_annotate_source(uri: SEXP, code: SEXP) -> anyhow::Result<SEXP> {
    let uri: String = RObject::view(uri).try_into()?;
    let code: String = RObject::view(code).try_into()?;

    let uri = Url::parse(&uri)?;

    let main = RMain::get();
    let mut dap_guard = main.debug_dap.lock().unwrap();

    // If there are no breakpoints for this file, return NULL to signal no
    // annotation needed
    let Some((_, breakpoints)) = dap_guard.breakpoints.get_mut(&uri) else {
        return Ok(harp::r_null());
    };
    if breakpoints.is_empty() {
        return Ok(harp::r_null());
    }

    let annotated = annotate_source(&code, &uri, breakpoints.as_mut_slice())?;
    Ok(RObject::try_from(annotated)?.sexp)
}

#[cfg(test)]
mod tests {
    use amalthea::wire::execute_request::CodeLocation;
    use amalthea::wire::execute_request::Position;
    use url::Url;

    use super::*;

    fn make_location(line: u32, character: u32) -> CodeLocation {
        CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position { line, character },
            end: Position { line, character },
        }
    }

    #[test]
    fn test_annotate_input_basic() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_shifted_line() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(10, 0);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_shifted_character() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(0, 5);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_shifted_line_and_character() {
        let code = "x <- 1\ny <- 2";
        let location = make_location(10, 5);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_existing_whitespace() {
        let code = "  x <- 1\n  y <- 2";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_existing_whitespace_shifted() {
        let code = "  x <- 1\n  y <- 2";
        let location = make_location(0, 2);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_existing_comment() {
        let code = "# comment\nx <- 1";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_empty_code() {
        let code = "";
        let location = make_location(0, 0);
        let result = annotate_input(code, location, None).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_input_with_breakpoint() {
        // Test the full annotate_input path with breakpoints.
        // Wrap in braces so breakpoints are valid.
        let code = "{\n0\n1\n2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 3,
                character: 0,
            },
            end: Position {
                line: 7,
                character: 1,
            },
        };
        // Breakpoint at document line 5 (code line 2, i.e., `1`)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 5,
            state: BreakpointState::Unverified,
        }];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        insta::assert_snapshot!(result);

        // Breakpoint line should remain in document coordinates
        assert_eq!(breakpoints[0].line, 5);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_single_line() {
        // Wrap in braces so breakpoints are valid (inside a brace list)
        let code = "{\nx <- 1\ny <- 2\nz <- 3\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2, // `y <- 2`
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_multiple() {
        // Wrap in braces so breakpoints are valid (inside a brace list)
        let code = "{\nx <- 1\ny <- 2\nz <- 3\nw <- 4\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 2, // `y <- 2`
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 4, // `w <- 4`
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_in_brace_list() {
        let code = "f <- function() {\n  x <- 1\n  y <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
    }

    #[test]
    fn test_inject_breakpoints_out_of_range() {
        let code = "x <- 1\ny <- 2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 10,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        // Should return unchanged code
        assert_eq!(result, code);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Verified));
    }

    #[test]
    fn test_inject_breakpoints_multiple_lists() {
        // This test has breakpoints in different parent lists:
        // - One in the outer brace list
        // - One in a nested brace list (inside function)
        // Wrap in braces so both breakpoints are valid
        let code = "{\nx <- 1\nf <- function() {\n  y <- 2\n  z <- 3\n}\nw <- 4\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 7,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 3, // Inside function - `y <- 2`
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 6, // In outer braces - `w <- 4`
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);
        // Both breakpoints are valid (inside brace lists)
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_with_blank_line() {
        // Test that blank lines before an anchor are preserved
        // Wrap in braces so breakpoints are valid
        let code = "{\nx <- 1\n\n\ny <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 4, // `y <- 2`
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_on_closing_brace() {
        // Breakpoint on a line with only `}` should be left unverified
        // (no executable code there)
        let code = "f <- function() {\n  x <- 1\n}\ny <- 2";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2, // The `}` line
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        // Should return unchanged code since breakpoint is invalid
        assert_eq!(result, code);
        // Marked as invalid
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_on_closing_brace_with_valid_breakpoint() {
        // One breakpoint on `}` (invalid) and one on valid code in outer braces
        // Wrap in braces so the second breakpoint is valid
        let code = "{\nf <- function() {\n  x <- 1\n}\ny <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 3, // The `}` line of the function - invalid
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 4, // `y <- 2` - in outer braces, valid
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // First breakpoint is invalid (on closing brace)
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
        // Second breakpoint is valid (in outer brace list)
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_before_within_after_nested() {
        // Comprehensive test with breakpoints:
        // - Before nested list (line 1: `x <- 1`) - in outer braces
        // - Within nested list (line 3: `y <- 2`) - inside function
        // - After nested list (line 6: `w <- 4`) - in outer braces
        // Wrap in braces so all breakpoints are valid
        let code = "{\nx <- 1\nf <- function() {\n  y <- 2\n  z <- 3\n}\nw <- 4\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 7,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 1, // `x <- 1` - in outer braces
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 3, // `y <- 2` - within nested function
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 3,
                line: 6, // `w <- 4` - in outer braces
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);
        // All breakpoints are valid (inside brace lists)
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[2].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_with_line_offset() {
        // Test that breakpoints work correctly when the code starts at a non-zero line
        // in the document. This simulates executing a selection from the middle of a file.
        // Wrap in braces so breakpoints are valid.
        //
        // The code represents lines 10-14 of the original document:
        // Line 10: {
        // Line 11: x <- 1
        // Line 12: y <- 2
        // Line 13: z <- 3
        // Line 14: }
        let code = "{\nx <- 1\ny <- 2\nz <- 3\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 10,
                character: 0,
            },
            end: Position {
                line: 14,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at document line 12 (which is code line 2, i.e., `y <- 2`)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 12,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // The breakpoint line should remain in document coordinates
        assert_eq!(breakpoints[0].line, 12);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_with_line_offset_nested() {
        // Test with line offset and nested braces
        let code = "f <- function() {\n  x <- 1\n  y <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 20,
                character: 0,
            },
            end: Position {
                line: 23,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at document line 22 (code line 2, i.e., `y <- 2`)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 22,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // The breakpoint line should remain in document coordinates
        assert_eq!(breakpoints[0].line, 22);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_doubly_nested_braces() {
        // Test with doubly nested braces: { { 1\n 2 } }
        // The inner expressions should be reachable for breakpoints
        let code = "{\n  {\n    1\n    2\n  }\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at line 2 (the `1` expression inside the inner braces)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // The breakpoint should be placed at line 2
        assert_eq!(breakpoints[0].line, 2);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_triply_nested_braces() {
        // Test with triply nested braces: { { { 1 } } }
        let code = "{\n  {\n    {\n      1\n    }\n  }\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 6,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at line 3 (the `1` expression inside the innermost braces)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 3,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // The breakpoint should be placed at line 3
        assert_eq!(breakpoints[0].line, 3);
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_nested_closing_brace_invalid() {
        // Breakpoint on inner closing brace should be invalid
        let code = "{\n  {\n    1\n  }\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Breakpoint at line 3 (the inner `}` line)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 3,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        // Should return unchanged code since breakpoint is invalid
        assert_eq!(result, code);
        // Marked as invalid
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_top_level_breakpoint_single_invalid() {
        // Top-level breakpoints are invalid (R can't step at top-level)
        let code = "x <- 1\ny <- 2\nz <- 3";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 1,
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        // Code unchanged since breakpoint is invalid
        assert_eq!(result, code);
        // Breakpoint marked as invalid
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_top_level_breakpoint_multiple_invalid() {
        // Multiple top-level breakpoints are all invalid
        let code = "x <- 1\ny <- 2\nz <- 3\nw <- 4";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 0,
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 2,
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        // Code unchanged since all breakpoints are invalid
        assert_eq!(result, code);
        // Both breakpoints marked as invalid
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert!(matches!(breakpoints[1].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_top_level_breakpoint_mixed_invalid_and_nested() {
        // Top-level breakpoints are invalid even when mixed with nested ones
        let code = "x <- 1\nf <- function() {\n  y <- 2\n}\nz <- 3";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 6,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 0, // `x <- 1` - top-level, invalid
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 2, // `y <- 2` - inside function, valid
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 3,
                line: 4, // `z <- 3` - top-level, invalid
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        // Code should contain breakpoint for nested expression only
        assert!(result.contains("base::.ark_breakpoint"));
        // Top-level breakpoints are invalid
        assert!(matches!(breakpoints[0].state, BreakpointState::Invalid));
        // Nested breakpoint is valid
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
        // Top-level breakpoint is invalid
        assert!(matches!(breakpoints[2].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_annotate_source_basic() {
        let code = "x <- 1\ny <- 2";
        let uri = Url::parse("file:///test.R").unwrap();
        let mut breakpoints = vec![];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_source_with_breakpoint() {
        let code = "foo <- function() {\n  x <- 1\n  y <- 2\n}\nbar <- 3";
        let uri = Url::parse("file:///test.R").unwrap();
        // Breakpoint at line 2 (inside the function, 0-indexed)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 1,
            state: BreakpointState::Unverified,
        }];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_source_multiple_expressions() {
        let code = "a <- 1\nb <- 2\nc <- 3";
        let uri = Url::parse("file:///test.R").unwrap();
        let mut breakpoints = vec![];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_source_multiline_expression() {
        let code = "foo <- function(x) {\n  x + 1\n}\nbar <- 2";
        let uri = Url::parse("file:///test.R").unwrap();
        let mut breakpoints = vec![];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_inject_breakpoints_if_else_both_branches() {
        let code = "if (TRUE) {\n  x <- 1\n} else {\n  y <- 2\n}";
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 1, // `x <- 1` in if branch
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 3, // `y <- 2` in else branch
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // Both breakpoints should be valid (not marked as invalid)
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "First breakpoint should not be invalid"
        );
        assert!(
            !matches!(breakpoints[1].state, BreakpointState::Invalid),
            "Second breakpoint should not be invalid"
        );
    }

    #[test]
    fn test_inject_breakpoints_multiple_invalid_closing_braces() {
        // Multiple breakpoints on closing braces should all be marked invalid
        // without re-traversing the tree for each one.
        let code = "{\n  f <- function() {\n    x <- 1\n  }\n}";
        // Line 0: {
        // Line 1:   f <- function() {
        // Line 2:     x <- 1
        // Line 3:   }        <- bp1 (closing brace of function)
        // Line 4: }          <- bp2 (closing brace of outer block)
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![
            Breakpoint {
                id: 1,
                line: 3, // closing brace of function
                state: BreakpointState::Unverified,
            },
            Breakpoint {
                id: 2,
                line: 4, // closing brace of outer block
                state: BreakpointState::Unverified,
            },
        ];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();

        // Code should be unchanged (no valid breakpoints)
        assert_eq!(result, code);

        // Both breakpoints should be marked invalid
        assert!(
            matches!(breakpoints[0].state, BreakpointState::Invalid),
            "First breakpoint on closing brace should be invalid"
        );
        assert!(
            matches!(breakpoints[1].state, BreakpointState::Invalid),
            "Second breakpoint on closing brace should be invalid"
        );
    }

    #[test]
    fn test_inject_breakpoints_empty_brace_sibling() {
        // Breakpoint on an empty brace block that's a sibling to other expressions
        let code = "{\n  x <- 1\n  {}\n}";
        // Line 0: {
        // Line 1:   x <- 1
        // Line 2:   {}      <- breakpoint here
        // Line 3: }
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2, // the empty {} expression
            state: BreakpointState::Unverified,
        }];

        let result = inject_breakpoints(code, location, &mut breakpoints, &line_index).unwrap();
        insta::assert_snapshot!(result);

        // Should anchor to the empty {} expression (it's a valid expression)
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on empty brace block should be valid"
        );
    }

    #[test]
    fn test_inject_breakpoints_nested_braces_same_line() {
        // Test breakpoints on nested brace structures
        let code = "{\n  {\n  }\n}";
        // Line 0: {       <- outer open
        // Line 1:   {     <- inner open (this is an expression in outer list)
        // Line 2:   }     <- inner close (invalid - closing brace)
        // Line 3: }       <- outer close (invalid - closing brace)
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        // Test 1: Breakpoint on inner brace open line (valid - anchors to inner {} expression)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 1,
            state: BreakpointState::Unverified,
        }];
        let result =
            inject_breakpoints(code, location.clone(), &mut breakpoints, &line_index).unwrap();
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on inner brace open should be valid"
        );
        assert!(result.contains(".ark_breakpoint"));

        // Test 2: Breakpoint on inner closing brace (invalid)
        let mut breakpoints = vec![Breakpoint {
            id: 2,
            line: 2,
            state: BreakpointState::Unverified,
        }];
        let result =
            inject_breakpoints(code, location.clone(), &mut breakpoints, &line_index).unwrap();
        assert!(
            matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on inner closing brace should be invalid"
        );
        assert_eq!(result, code);

        // Test 3: Breakpoint on outer closing brace (invalid)
        let mut breakpoints = vec![Breakpoint {
            id: 3,
            line: 3,
            state: BreakpointState::Unverified,
        }];
        let result =
            inject_breakpoints(code, location.clone(), &mut breakpoints, &line_index).unwrap();
        assert!(
            matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on outer closing brace should be invalid"
        );
        assert_eq!(result, code);
    }

    #[test]
    fn test_inject_breakpoints_double_braces_same_lines() {
        // Test breakpoints with {{ on one line and }} on another
        let code = "{{\n}}";
        // Line 0: {{    <- outer and inner open
        // Line 1: }}    <- inner and outer close
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 2,
            },
        };
        let line_index = LineIndex::new(code);

        // Test 1: Breakpoint on line 0 (valid - anchors to inner {} expression)
        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 0,
            state: BreakpointState::Unverified,
        }];
        let result =
            inject_breakpoints(code, location.clone(), &mut breakpoints, &line_index).unwrap();
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on {{ line should be valid"
        );
        assert!(result.contains(".ark_breakpoint"));

        // Test 2: Breakpoint on line 1 (invalid - closing braces)
        let mut breakpoints = vec![Breakpoint {
            id: 2,
            line: 1,
            state: BreakpointState::Unverified,
        }];
        let result =
            inject_breakpoints(code, location.clone(), &mut breakpoints, &line_index).unwrap();
        assert!(
            matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on }} line should be invalid"
        );
        assert_eq!(result, code);
    }

    #[test]
    fn test_inject_breakpoints_inside_multiline_call() {
        // Test breakpoint placed on a line inside a multi-line call expression
        // The breakpoint is on the argument line, not the start of the expression
        let code = "{\n  foo(\n    1\n  )\n}";
        // Line 0: {
        // Line 1:   foo(
        // Line 2:     1      <- breakpoint here
        // Line 3:   )
        // Line 4: }
        let location = CodeLocation {
            uri: Url::parse("file:///test.R").unwrap(),
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 1,
            },
        };
        let line_index = LineIndex::new(code);

        let mut breakpoints = vec![Breakpoint {
            id: 1,
            line: 2, // Inside the foo() call, on the argument line
            state: BreakpointState::Unverified,
        }];

        let result =
            inject_breakpoints(code, location.clone(), &mut breakpoints, &line_index).unwrap();

        // Breakpoint inside a multi-line expression (not at its start) is invalid
        assert!(
            matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint inside multi-line call should be invalid"
        );
        assert_eq!(result, code, "Invalid breakpoint should not modify code");
    }
}
