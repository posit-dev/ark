//
// console_annotate.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use aether_syntax::RBracedExpressions;
use aether_syntax::RExpressionList;
use aether_syntax::RLanguage;
use aether_syntax::RSyntaxElement;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use amalthea::wire::execute_request::CodeLocation;
use anyhow::anyhow;
use biome_line_index::LineIndex;
use biome_rowan::AstNode;
use biome_rowan::SyntaxRewriter;
use biome_rowan::TriviaPieceKind;
use biome_rowan::VisitNodeSignal;
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

// Breakpoint injection at parse-time rather than eval-time.
//
// Traditional approaches (like RStudio's) inject `browser()` calls into live
// objects at eval-time. This is fragile: you must find all copies of the object
// (explicit/implicit exports, method tables, etc.), preserve source references
// after injection, and handle edge cases for each object system (R6, S7, ...).
// These injection routines are ad hoc and require ongoing maintenance to add
// missing injection places as the ecosystem evolves.
//
// By injecting breakpoints before R ever sees the code, we sidestep all of
// this. R only evaluates code that already contains breakpoint calls, so we
// never miss copies and source references are correct from the start.
//
// A breakpoint injected on `expression_to_break_on` looks like this:
//
// ```r
// .ark_auto_step(.ark_breakpoint(browser(), "*url*", "*id*"))
// #line *line* "*url*"
// expression_to_break_on
// ```
//
// - `.ark_auto_step()` is an identity function that serves as sentinel when R
//   steps through code. If the user steps on an injected breakpoint, we detect
//   the auto-step call in the `debug at` message emitted by R and automatically
//   step over it (i.e. call `n`).
//
// - `.ark_breakpoint()` takes a `browser()` call promised in the current
//   environment, a URL, and the breakpoint's unique ID. It only forces the
//   browser argument if the breakpoint is active. Since the argument is promised
//   in the call-site environment, this cause R to mark that environment as being
//   debugged.
//
//   It does not stop quite at the right place though, inside the
//   `.ark_breakpoint()` wrapper, with `.ark_auto_step()` on the stack as well. To
//   solve this, there is a second condition triggering auto-stepping in
//   ReadConsole: if the function of the top stack frame is `.ark_breakpoint()`
//   (which we detect through a class assigned to the function), then we
//   auto-step. This causes R to resume evaluation. Since the call-site
//   environment is being debugged, it stops at the next expression automatically,
//   in this case `expression_to_break_on`.
//
// - The `#line` directive right above `expression_to_break_on` maps the source
//   references to the original location in the source document. When R stops on
//   the expression, it emits the original location, allowing the DAP to
//   communicate the appropriate stopping place to the frontend.

// Source instrumentation
//
// `base::source()` and `devtools::load_all()` need two things:
//
// - Breakpoint injection as described above.
//
// - Top-level adjustments so it's possible to step through a script or
//   top-level package file (the latter is rarely useful but is a side benefit
//   from using the same implementation as `source()`).
//
// If the sourced file looks like:
//
// ```r
// 1
// 2
// ```
//
// The instrumented version ends up as:
//
// ```r
// {
// #line 1 "file:///file.R"
// 1
// base::.ark_auto_step(base::.ark_verify_breakpoints_range("file:///test.R", 1L, 2L))
// #line 2 "file:///file.R"
// 2
// base::.ark_auto_step(base::.ark_verify_breakpoints_range("file:///test.R", 2L, 3L))
// }
// ```
//
// - The whole source is wrapped in `{}` to allow R to step through the code.
// - Line directives map each expression to original source.
// - An auto-stepped `.ark_verify_breakpoints_range()` call after each
//   expression lets the DAP know that any breakpoints spanned by the last
//   expression are now "verified", i.e. the breakpoints have been injected and
//   the code containing them has been evaluated.

// Breakpoint injection uses a Biome `SyntaxRewriter`, a tree visitor with
// preorder and postorder hooks that allows replacing nodes on the way out.
//
// - Preorder (`visit_node`): Cache line information for braced expressions.
//   We record where each expression starts and its line range. This must be
//   done before any modifications because the postorder hook sees a partially
//   rebuilt tree with shifted token offsets.
//
// - Postorder (`visit_node_post`): Process expression lists bottom-up. For
//   lists inside braces, we inject breakpoint calls, add `#line` directives,
//   and mark remaining breakpoints (e.g. on closing braces) as invalid. The
//   cached line info represents original source positions, which is exactly
//   what we need for anchoring breakpoints to document lines.
//
// Note: The line info cached in pre-order visit can potentially become stale in
// the post-order hook, since the latter is operating on a reconstructed tree.
// However, we only use the cached info to reason about where original
// expressions lived in the source document, not where they end up in the
// rewritten tree. This is safe as long as we don't reorder or remove
// expressions (we only inject siblings and trivia).
//
// We use `SyntaxRewriter` instead of `BatchMutation` because the latter:
//
// - Doesn't handle insertions in lists (only replacements).
//
// - Doesn't handle nested changes in a node that is later replaced. For example:
//
//   ```r
//   {     # BP 1
//      1  # BP 2
//   }
//   ```
//
//   BP 2 causes changes inside the braces. Then BP 1 causes the whole brace
//   expression to be replaced with a variant that has a `#line` directive.
//   BatchMutation can't express both changes because it takes modifications
//   upfront. `SyntaxRewriter` lets us replace nodes bottom-up as we go.

/// Annotate console input for `ReadConsole`.
///
/// - Adds a `#line` directive to map the code back to its document location.
/// - Adds leading whitespace to align with the original character offset.
/// - Injects breakpoint calls if any breakpoints are set.
pub(crate) fn annotate_input(
    code: &str,
    location: CodeLocation,
    breakpoints: Option<&mut [Breakpoint]>,
) -> anyhow::Result<String> {
    // First, inject breakpoints into the original code. This must happen before
    // we add the outer line directive, otherwise the coordinates of inner line
    // directives are shifted by 1 line.
    let annotated_code = if let Some(breakpoints) = breakpoints {
        let root = aether_parser::parse(code, Default::default()).tree();
        let line_index = LineIndex::new(code);

        // The line offset is `doc_line = code_line + line_offset`.
        // Code line 0 corresponds to document line `location.start.line`.
        let line_offset = location.start.line as i32;

        let mut rewriter =
            AnnotationRewriter::new(&location.uri, breakpoints, line_offset, &line_index);
        let out = rewriter.transform(root.syntax().clone());

        if let Some(err) = rewriter.take_err() {
            return Err(err);
        }

        out.to_string()
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
        "{line_directive}\n{leading_padding}{annotated_code}"
    ))
}

/// Annotate source code for `source()` and `pkgload::load_all()`.
///
/// - Wraps the whole source in a `{}` block first. This allows R to step through
///   top-level expressions and makes all breakpoints "nested" inside braces.
/// - Injects breakpoint calls (`.ark_auto_step(.ark_breakpoint(...))`) at
///   breakpoint locations.
/// - Injects verification calls (`.ark_auto_step(.ark_verify_breakpoints_range(...))`)
///   after expressions containing breakpoints.
/// - `#line` directives after injected calls to restore correct line mapping.
pub(crate) fn annotate_source(
    code: &str,
    uri: &Url,
    breakpoints: &mut [Breakpoint],
) -> anyhow::Result<String> {
    // Wrap code in braces first. This:
    // 1. Allows R to step through top-level expressions
    // 2. Makes all breakpoints valid (they're now inside braces, at top-level they'd be invalid)
    // This enables uniform treatment by `AnnotationRewriter` for input and source cases.
    let wrapped = format!("{{\n{code}\n}}");
    let line_index = LineIndex::new(&wrapped);

    let root = aether_parser::parse(&wrapped, Default::default()).tree();

    // `line_offset` = -1 because:
    // - Wrapped line 0 is `{`
    // - Wrapped line 1 is original line 0
    // - doc_line = code_line + line_offset = code_line - 1
    let line_offset: i32 = -1;

    let mut rewriter = AnnotationRewriter::new(uri, breakpoints, line_offset, &line_index);
    let transformed = rewriter.transform(root.syntax().clone());

    if let Some(err) = rewriter.take_err() {
        return Err(err);
    }

    let transformed_code = transformed.to_string();

    // Add a trailing verify call to handle any injected breakpoint in trailing
    // position. Normally we'd inject a verify call as well a line directive
    // that ensures source references remain correct after the verify call.
    // But for the last expression in a list, there is no sibling node to attach
    // the line directive trivia to. So, instead of adding a verify call, we
    // rely on verification in a parent list instead. If trailing, there won't
    // be any verification calls at all though, so we manually add one there:
    //
    // ```r
    // {
    //    foo({
    //      .ark_auto_step(.breakpoint(...))
    //      #line ...
    //      expr
    //    })
    // }
    // .ark_auto_step(.ark_verify_breakpoints(...))   # <- Manual injection
    // ```
    //
    // This is unconditional for simplicity.
    let last_line = code.lines().count() as u32;
    let trailing_verify = format_verify_call(uri, &(0..last_line));

    Ok(format!("{}\n{trailing_verify}\n", transformed_code.trim()))
}

/// Rewriter that handles all code annotation inside braced expression lists:
/// - Breakpoint calls on statements
/// - Verification calls after statements containing breakpoints
/// - `#line` directives after injected calls to restore sourceref bookkeeping
struct AnnotationRewriter<'a> {
    uri: &'a Url,
    /// Breakpoints in document coordinates, will be mutated to mark invalid ones
    breakpoints: &'a mut [Breakpoint],
    /// Offset for coordinate conversion: doc_line = code_line + line_offset
    line_offset: i32,
    /// Line index for the parsed code
    line_index: &'a LineIndex,
    /// Set of breakpoint IDs that have been consumed (placed in nested lists)
    consumed: std::collections::HashSet<i64>,
    /// Stack tracking braced expression context. Each entry contains precomputed
    /// line information captured before child transformations (which can corrupt ranges).
    brace_stack: Vec<BraceFrame>,
    /// First error encountered during transformation (if any)
    err: Option<anyhow::Error>,
}

/// Holds precomputed line information for a braced expression list.
/// Captured on entry to a braced expression since line info becomes unreliable
/// after child transformations.
struct BraceFrame {
    /// Code line of the opening `{`
    brace_code_line: u32,
    /// Line info for each expression (indexed by slot position)
    expr_info: Vec<ExprLineInfo>,
}

/// Precomputed line information for a single expression in a braced list.
struct ExprLineInfo {
    /// Code line where the expression starts (from first token)
    start: u32,
    /// Code line range [start, end) for the expression
    range: std::ops::Range<u32>,
}

impl<'a> AnnotationRewriter<'a> {
    fn new(
        uri: &'a Url,
        breakpoints: &'a mut [Breakpoint],
        line_offset: i32,
        line_index: &'a LineIndex,
    ) -> Self {
        // Sort so that `find_breakpoint_for_expr` (which uses `position()`) finds
        // the earliest-line breakpoint first when multiple could match
        breakpoints.sort_by_key(|bp| bp.line);

        Self {
            uri,
            breakpoints,
            line_offset,
            line_index,
            consumed: std::collections::HashSet::new(),
            brace_stack: Vec::new(),
            err: None,
        }
    }

    fn take_err(&mut self) -> Option<anyhow::Error> {
        self.err.take()
    }

    fn fail(&mut self, err: anyhow::Error, node: RSyntaxNode) -> RSyntaxNode {
        if self.err.is_none() {
            self.err = Some(err);
        }
        node
    }

    /// Convert code line to document line. Can be negative for the wrapper
    /// brace in `annotate_source().
    fn to_doc_line(&self, code_line: u32) -> i32 {
        code_line as i32 + self.line_offset
    }

    /// Check if a breakpoint is available (not consumed and not invalid)
    fn is_available(&self, bp: &Breakpoint) -> bool {
        !self.consumed.contains(&bp.id) && !matches!(bp.state, BreakpointState::Invalid)
    }

    /// Find all available breakpoints that anchor to this expression: At or
    /// after the previous expression's end, up to and including the
    /// expression's last line. Returns the indices of all matching breakpoints.
    fn match_breakpoints(
        &self,
        prev_doc_end: Option<i32>,
        expr_last_line: i32,
    ) -> anyhow::Result<Vec<usize>> {
        if expr_last_line < 0 {
            return Err(anyhow!(
                "Unexpected negative `expr_last_line` ({expr_last_line})"
            ));
        }
        let expr_last_line = expr_last_line as u32;

        let result: Vec<usize> = self
            .breakpoints
            .iter()
            .enumerate()
            .filter_map(|(idx, bp)| {
                if !self.is_available(bp) {
                    return None;
                }

                // Breakpoint is after this expression
                if bp.line > expr_last_line {
                    return None;
                }

                // There is no previous expression so that's a match
                let Some(prev_doc_end) = prev_doc_end else {
                    return Some(idx);
                };

                // Breakpoint must be after the end of the previous expression.
                // Note we allow blank lines between expressions to anchor to
                // the next expression.
                if (bp.line as i32) >= prev_doc_end {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
        Ok(result)
    }

    /// Check if any breakpoint (including consumed ones) falls within the line
    /// range [start, end) in document coordinates. This is used to determine
    /// whether a statement contains breakpoints and thus needs to be followed
    /// by a verify call.
    fn has_breakpoints_in_range(&self, start: i32, end: i32) -> bool {
        self.breakpoints.iter().any(|bp| {
            let bp_line = bp.line as i32;
            !matches!(bp.state, BreakpointState::Invalid) && bp_line >= start && bp_line < end
        })
    }
}

impl SyntaxRewriter for AnnotationRewriter<'_> {
    type Language = RLanguage;

    fn visit_node(&mut self, node: RSyntaxNode) -> VisitNodeSignal<Self::Language> {
        if self.err.is_some() {
            // Something is wrong but we can't short-circuit the visit. Just
            // visit nodes until exhaustion.
            return VisitNodeSignal::Traverse(node);
        }

        // Track `BraceFrame` information when we enter a braced expression.
        // This must be done on entry, since the exit hook only sees the
        // (partially) rebuilt tree with invalid line information.
        //
        // Note: we intentionally cache line info from the original parse tree.
        // Downstream injections (breakpoint calls, `#line` trivia, verify calls)
        // can change token offsets and make line lookups on the rebuilt nodes
        // unreliable, but they do not change where the original expressions
        // lived in the source. We only rely on these cached source positions
        // when anchoring and invalidating breakpoints, tasks for which we need
        // the _original_ coordinates, not the new ones.
        if let Some(braced) = RBracedExpressions::cast(node.clone()) {
            let Some(brace_code_line) = first_token_code_line(&node, self.line_index) else {
                self.err = Some(anyhow!("Failed to get line for opening brace"));
                return VisitNodeSignal::Traverse(node);
            };

            let mut expr_info = Vec::new();

            for expr in braced.expressions() {
                let Some(start) = first_token_code_line(expr.syntax(), self.line_index) else {
                    self.err = Some(anyhow!("Failed to get start line for expression"));
                    return VisitNodeSignal::Traverse(node);
                };
                let range = match text_trimmed_line_range(expr.syntax(), self.line_index) {
                    Ok(range) => range,
                    Err(err) => {
                        self.err = Some(err);
                        return VisitNodeSignal::Traverse(node);
                    },
                };

                expr_info.push(ExprLineInfo { start, range });
            }

            self.brace_stack.push(BraceFrame {
                brace_code_line,
                expr_info,
            });
        }

        VisitNodeSignal::Traverse(node)
    }

    fn visit_node_post(&mut self, node: RSyntaxNode) -> RSyntaxNode {
        if self.err.is_some() {
            // Something is wrong but we can't short-circuit the visit. Just
            // visit nodes until exhaustion.
            return node;
        }

        // Only process expression lists
        if node.kind() != RSyntaxKind::R_EXPRESSION_LIST {
            return node;
        }

        // Note we assume that only braced expressions and the root list have
        // `R_EXPRESSION_LIST`, which is the case in our syntax
        if let Some(frame) = self.brace_stack.pop() {
            // Empty braces have no expressions to break on; any breakpoints
            // in this range belong to an outer scope
            if frame.expr_info.is_empty() {
                return node;
            }

            // Brace range in document coordinates. Since we checked for empty
            // expr_info above, first/last are guaranteed to exist.
            let Some(last_info) = frame.expr_info.last() else {
                return self.fail(anyhow!("expr_info unexpectedly empty"), node);
            };
            let Some(first_info) = frame.expr_info.first() else {
                return self.fail(anyhow!("expr_info unexpectedly empty"), node);
            };

            let brace_doc_start = self.to_doc_line(frame.brace_code_line);
            let brace_doc_end = self.to_doc_line(last_info.range.end);
            let first_expr_doc_start = self.to_doc_line(first_info.start);

            // Annotate statements in the braced list
            let result = self.annotate_braced_list(node, frame.brace_code_line, frame.expr_info);

            // Mark any remaining breakpoints in this brace range as invalid
            let invalidation_floor = breakpoint_floor(brace_doc_start, first_expr_doc_start);
            self.mark_remaining_breakpoints_invalid(Some(invalidation_floor), Some(brace_doc_end));

            result
        } else {
            // We're at the root expression list, mark all remaining breakpoints as invalid
            self.mark_remaining_breakpoints_invalid(None, None);
            node
        }
    }
}

impl AnnotationRewriter<'_> {
    /// Annotate an expression list inside braces with breakpoints, `#line`
    /// directives, and verification calls.
    fn annotate_braced_list(
        &mut self,
        list_node: RSyntaxNode,
        brace_code_line: u32,
        expr_info: Vec<ExprLineInfo>,
    ) -> RSyntaxNode {
        let Some(list) = RExpressionList::cast(list_node.clone()) else {
            return list_node;
        };

        let elements: Vec<_> = list.into_iter().collect();
        if elements.is_empty() {
            return list_node;
        }

        // Convert brace code line to document coordinates. This is the floor
        // for breakpoint matching, breakpoints before this line belong to an
        // outer scope, not this braced list. Note that due to the injected
        // wrapper braces in `annotate_source()`, this can be -1 (before any doc
        // line).
        let brace_doc_start: i32 = self.to_doc_line(brace_code_line);

        let mut result_slots: Vec<Option<RSyntaxElement>> = Vec::new();
        let mut needs_line_directive = false;

        let first_expr_doc_start = expr_info
            .first()
            .map(|info| self.to_doc_line(info.start))
            .unwrap_or(brace_doc_start);
        let mut prev_doc_end: Option<i32> =
            Some(breakpoint_floor(brace_doc_start, first_expr_doc_start));

        for (i, expr) in elements.iter().enumerate() {
            // Use precomputed line info captured on the preorder visit, when
            // positions were still valid
            let Some(info) = expr_info.get(i) else {
                return self.fail(anyhow!("Missing line info for expression {i}"), list_node);
            };

            let expr_doc_start = self.to_doc_line(info.start);
            let expr_doc_end = self.to_doc_line(info.range.end);

            // Find all breakpoints that anchor to this expression:
            // - At or after the previous expression's end
            // - At or before the expression's last line (expr_doc_end - 1, since end is exclusive)
            // This includes breakpoints on blank lines before the expression and
            // breakpoints inside multiline expressions (which all anchor to the start).
            let bp_indices = match self.match_breakpoints(prev_doc_end, expr_doc_end - 1) {
                Ok(indices) => indices,
                Err(e) => return self.fail(e, list_node),
            };

            if !bp_indices.is_empty() {
                // Use the first breakpoint's id for the injected call
                let first_bp_id = self.breakpoints[bp_indices[0]].id;

                // Update all matching breakpoints: anchor to expr start and mark consumed
                for &bp_idx in &bp_indices {
                    let bp = &mut self.breakpoints[bp_idx];
                    bp.line = expr_doc_start as u32;
                    self.consumed.insert(bp.id);
                }

                // Inject a single breakpoint call for all matching breakpoints
                // (all breakpoints are shown at the same location in the
                // frontend, once verified)
                let breakpoint_call = create_breakpoint_call(self.uri, first_bp_id);
                result_slots.push(Some(breakpoint_call.into()));

                // We've injected an expression so we'll need to fix sourcerefs
                // with a line directive
                needs_line_directive = true;
            }

            // There are two reasons we might need a line directive:
            // - We've just injected a breakpoint
            // - We've injected a verify call at last iteration
            let expr_node = if needs_line_directive {
                match add_line_directive_to_node(expr.syntax(), expr_doc_start, self.uri) {
                    Ok(n) => n,
                    Err(e) => return self.fail(e, list_node),
                }
            } else {
                expr.syntax().clone()
            };
            result_slots.push(Some(expr_node.into()));

            // If this expression's range contains any breakpoints, we inject a
            // verify call right after it to ensure that they are immediately
            // verified after stepping over this expression. The very last
            // expression in the list is an exception. We don't inject a verify
            // call because we have nowhere to attach a corresponding line
            // directive. Instead we rely on the parent list to verify.
            let is_last = i == elements.len() - 1;
            if !is_last && self.has_breakpoints_in_range(expr_doc_start, expr_doc_end) {
                let start_u32 = expr_doc_start.max(0) as u32;
                let end_u32 = expr_doc_end.max(0) as u32;
                let verify_call = create_verify_call(self.uri, &(start_u32..end_u32));
                result_slots.push(Some(verify_call.into()));

                // Next expression will need a line directive no matter what
                // (even if there is no injected breakpoint)
                needs_line_directive = true;
            } else {
                needs_line_directive = false;
            }

            prev_doc_end = Some(expr_doc_end);
        }

        // Replace all slots with the new list
        let slot_count = list_node.slots().count();
        list_node.splice_slots(0..slot_count, result_slots)
    }

    /// Mark remaining unconsumed breakpoints as invalid within the given range.
    /// If range bounds are None, all remaining breakpoints are marked invalid.
    fn mark_remaining_breakpoints_invalid(&mut self, start: Option<i32>, end: Option<i32>) {
        for bp in self.breakpoints.iter_mut() {
            let is_available =
                !self.consumed.contains(&bp.id) && !matches!(bp.state, BreakpointState::Invalid);
            if !is_available {
                continue;
            }

            let bp_line = bp.line as i32;
            let in_range =
                start.map_or(true, |s| bp_line >= s) && end.map_or(true, |e| bp_line <= e);
            if in_range {
                bp.state = BreakpointState::Invalid;
            }
        }
    }
}

/// Compute the floor line for breakpoint matching in a braced list. When
/// content starts on a later line than the brace, we use `brace_doc_start + 1`
/// to avoid claiming breakpoints on the brace line, as those belong to the
/// parent scope.
fn breakpoint_floor(brace_doc_start: i32, first_expr_doc_start: i32) -> i32 {
    if first_expr_doc_start > brace_doc_start {
        brace_doc_start + 1
    } else {
        brace_doc_start
    }
}

/// Returns the code line of the node's first token.
fn first_token_code_line(node: &RSyntaxNode, line_index: &LineIndex) -> Option<u32> {
    let token = node.first_token()?;
    let offset = token.text_trimmed_range().start();
    line_index.line_col(offset).map(|lc| lc.line)
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
    line: i32,
    uri: &Url,
) -> anyhow::Result<RSyntaxNode> {
    if line < 0 {
        return Err(anyhow!(
            "Line directive line is negative ({line}), this shouldn't happen"
        ));
    }
    let line = line as u32;

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
    let code = format!("\n{}\n", format_verify_call(uri, line_range));
    aether_parser::parse(&code, Default::default()).syntax()
}

/// Formats a verify call as a string. Takes 0-indexed line range.
fn format_verify_call(uri: &Url, line_range: &std::ops::Range<u32>) -> String {
    format!(
        "base::{AUTO_STEP_FUNCTION}(base::.ark_verify_breakpoints_range(\"{}\", {}L, {}L))",
        uri,
        line_range.start + 1,
        line_range.end + 1
    )
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
        let mut breakpoints = vec![Breakpoint::new(1, 5, BreakpointState::Unverified)];

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
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 2, BreakpointState::Unverified),
            Breakpoint::new(2, 4, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![Breakpoint::new(1, 10, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        // annotate_input always adds #line directive for srcref mapping
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 3, BreakpointState::Unverified),
            Breakpoint::new(2, 6, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        insta::assert_snapshot!(result);
        // Both breakpoints are valid (inside brace lists)
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert!(!matches!(breakpoints[1].state, BreakpointState::Invalid));
    }

    #[test]
    fn test_inject_breakpoints_inside_multiline_expr_anchors_to_start() {
        // A breakpoint on an intermediate line of a multiline expression should
        // anchor to the start of that expression.
        // Lines:
        //   0: {
        //   1:   x +
        //   2:     y
        //   3:   z
        //   4: }
        let code = "{\n  x +\n    y\n  z\n}";
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
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        insta::assert_snapshot!(result);
        // Breakpoint inside multiline expression should anchor to expression start
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert_eq!(breakpoints[0].line, 1); // Anchored to line 1 (x +)
    }

    #[test]
    fn test_inject_breakpoints_on_blank_line_anchors_to_next() {
        // A breakpoint on a blank line between expressions should anchor to
        // the next expression.
        // Lines:
        //   0: {
        //   1:   x <- 1
        //   2:   (blank)
        //   3:   y <- 2
        //   4: }
        let code = "{\n  x <- 1\n\n  y <- 2\n}";
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
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        insta::assert_snapshot!(result);
        // Breakpoint on blank line should anchor to next expression (valid)
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        // Line should be updated to the actual anchor position (line 3)
        assert_eq!(breakpoints[0].line, 3);
    }

    #[test]
    fn test_multiple_breakpoints_collapse_to_same_line() {
        // Multiple breakpoints matching the same expression should all anchor
        // to the expression start, but only one breakpoint call is injected.
        // Lines:
        //   0: {
        //   1:   (blank)
        //   2:   foo(
        //   3:     1
        //   4:   )
        //   5: }
        let code = "{\n\n  foo(\n    1\n  )\n}";
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
        // Three breakpoints: blank line, expression start, and inside expression
        let mut breakpoints = vec![
            Breakpoint::new(1, 1, BreakpointState::Unverified),
            Breakpoint::new(2, 2, BreakpointState::Unverified),
            Breakpoint::new(3, 3, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        insta::assert_snapshot!(result);

        // All breakpoints should be valid and anchored to line 2 (expression start)
        for bp in &breakpoints {
            assert!(
                !matches!(bp.state, BreakpointState::Invalid),
                "Breakpoint {} should be valid",
                bp.id
            );
            assert_eq!(bp.line, 2, "Breakpoint {} should anchor to line 2", bp.id);
        }

        // Only one breakpoint call should be injected (count occurrences)
        let bp_call_count = result.matches(".ark_breakpoint").count();
        assert_eq!(
            bp_call_count, 1,
            "Only one breakpoint call should be injected"
        );
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
        let mut breakpoints = vec![Breakpoint::new(1, 4, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        // annotate_input always adds #line directive for srcref mapping
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 3, BreakpointState::Unverified),
            Breakpoint::new(2, 4, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 1, BreakpointState::Unverified),
            Breakpoint::new(2, 3, BreakpointState::Unverified),
            Breakpoint::new(3, 6, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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

        // Breakpoint at document line 12 (which is code line 2, i.e., `y <- 2`)
        let mut breakpoints = vec![Breakpoint::new(1, 12, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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

        // Breakpoint at document line 22 (code line 2, i.e., `y <- 2`)
        let mut breakpoints = vec![Breakpoint::new(1, 22, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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

        // Breakpoint at line 2 (the `1` expression inside the inner braces)
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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

        // Breakpoint at line 3 (the `1` expression inside the innermost braces)
        let mut breakpoints = vec![Breakpoint::new(1, 3, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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

        // Breakpoint at line 3 (the inner `}` line)
        let mut breakpoints = vec![Breakpoint::new(1, 3, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        // annotate_input always adds #line directive for srcref mapping
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);
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
        let mut breakpoints = vec![Breakpoint::new(1, 1, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        // annotate_input always adds #line directive for srcref mapping
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 0, BreakpointState::Unverified),
            Breakpoint::new(2, 2, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
        // annotate_input always adds #line directive for srcref mapping
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 0, BreakpointState::Unverified),
            Breakpoint::new(2, 2, BreakpointState::Unverified),
            Breakpoint::new(3, 4, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![Breakpoint::new(1, 1, BreakpointState::Unverified)];
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
    fn test_annotate_source_top_level_breakpoint() {
        let code = "x <- 1\ny <- 2\nz <- 3";
        let uri = Url::parse("file:///test.R").unwrap();
        // Breakpoints on top-level expressions (lines 0, 1, 2 in 0-indexed)
        let mut breakpoints = vec![
            Breakpoint::new(1, 0, BreakpointState::Unverified),
            Breakpoint::new(2, 2, BreakpointState::Unverified),
        ];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();

        // Top-level breakpoints should be valid in annotate_source (code is wrapped in braces)
        assert_eq!(breakpoints[0].state, BreakpointState::Unverified);
        assert_eq!(breakpoints[1].state, BreakpointState::Unverified);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_source_multiple_breakpoints_inside_braces() {
        // Breakpoints at lines 1 and 2 (1-based), i.e. lines 0 and 1 (0-indexed)
        // Line 0: `{`
        // Line 1: `  1`
        let code = "{\n  1\n  2\n}\n\n2";
        let uri = Url::parse("file:///test.R").unwrap();
        let mut breakpoints = vec![
            Breakpoint::new(1, 0, BreakpointState::Unverified),
            Breakpoint::new(2, 1, BreakpointState::Unverified),
        ];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();

        // Breakpoint 1 should be at line 0 (the `{`)
        // Breakpoint 2 should be at line 1 (the `1`)
        assert_eq!(breakpoints[0].line, 0);
        assert_eq!(breakpoints[1].line, 1);
        assert_eq!(breakpoints[0].state, BreakpointState::Unverified);
        assert_eq!(breakpoints[1].state, BreakpointState::Unverified);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_source_breakpoint_on_opening_brace() {
        // Breakpoint on line 0 (the `{` line) should anchor to the braced expression,
        // not dive into the nested list and anchor to line 1.
        let code = "{\n  1\n  2\n}\n\n2";
        let uri = Url::parse("file:///test.R").unwrap();
        let mut breakpoints = vec![Breakpoint::new(1, 0, BreakpointState::Unverified)];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();

        // Breakpoint should remain at line 0, not shifted to line 1
        assert_eq!(breakpoints[0].line, 0);
        assert_eq!(breakpoints[0].state, BreakpointState::Unverified);
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_annotate_source_breakpoint_on_function_definition_line() {
        // Breakpoint on the function definition line (which includes the opening `{`)
        // should anchor to the assignment expression, not dive into the function body.
        // Line 0: `f <- function(x) {`
        // Line 1: `  1`
        // Line 2: `}`
        let code = "f <- function(x) {\n  1\n}";
        let uri = Url::parse("file:///test.R").unwrap();
        let mut breakpoints = vec![
            Breakpoint::new(1, 0, BreakpointState::Unverified),
            Breakpoint::new(2, 1, BreakpointState::Unverified),
        ];
        let result = annotate_source(code, &uri, &mut breakpoints).unwrap();

        // Breakpoint 1 should remain at line 0 (the function definition)
        // Breakpoint 2 should be at line 1 (inside the function body)
        assert_eq!(breakpoints[0].line, 0);
        assert_eq!(breakpoints[1].line, 1);
        assert_eq!(breakpoints[0].state, BreakpointState::Unverified);
        assert_eq!(breakpoints[1].state, BreakpointState::Unverified);
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 1, BreakpointState::Unverified),
            Breakpoint::new(2, 3, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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
        let mut breakpoints = vec![
            Breakpoint::new(1, 3, BreakpointState::Unverified),
            Breakpoint::new(2, 4, BreakpointState::Unverified),
        ];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();

        // annotate_input always adds #line directive for srcref mapping
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);

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
        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location, Some(&mut breakpoints)).unwrap();
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

        // Test 1: Breakpoint on inner brace open line (valid - anchors to inner {} expression)
        let mut breakpoints = vec![Breakpoint::new(1, 1, BreakpointState::Unverified)];
        let result = annotate_input(code, location.clone(), Some(&mut breakpoints)).unwrap();
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on inner brace open should be valid"
        );
        assert!(result.contains(".ark_breakpoint"));

        // Test 2: Breakpoint on inner closing brace (anchors to inner {} expression start)
        let mut breakpoints = vec![Breakpoint::new(2, 2, BreakpointState::Unverified)];
        let result = annotate_input(code, location.clone(), Some(&mut breakpoints)).unwrap();
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on inner closing brace should anchor to inner {{ expression"
        );
        assert_eq!(breakpoints[0].line, 1, "Should anchor to line 1");
        assert!(result.contains(".ark_breakpoint"));

        // Test 3: Breakpoint on outer closing brace (invalid - not part of any expression in the list)
        let mut breakpoints = vec![Breakpoint::new(3, 3, BreakpointState::Unverified)];
        let result = annotate_input(code, location.clone(), Some(&mut breakpoints)).unwrap();
        assert!(
            matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on outer closing brace should be invalid"
        );
        let expected = format!("#line 1 \"file:///test.R\"\n{code}");
        assert_eq!(result, expected);
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

        // Test 1: Breakpoint on line 0 (valid - anchors to inner {} expression)
        let mut breakpoints = vec![Breakpoint::new(1, 0, BreakpointState::Unverified)];
        let result = annotate_input(code, location.clone(), Some(&mut breakpoints)).unwrap();
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on {{ line should be valid"
        );
        assert!(result.contains(".ark_breakpoint"));

        // Test 2: Breakpoint on line 1 (anchors to inner {} which spans lines 0-1)
        let mut breakpoints = vec![Breakpoint::new(2, 1, BreakpointState::Unverified)];
        let result = annotate_input(code, location.clone(), Some(&mut breakpoints)).unwrap();
        // Breakpoint on }} line anchors to the inner {} expression start (line 0)
        assert!(
            !matches!(breakpoints[0].state, BreakpointState::Invalid),
            "Breakpoint on }} line should anchor to inner {{ expression"
        );
        assert_eq!(breakpoints[0].line, 0, "Should anchor to line 0");
        assert!(result.contains(".ark_breakpoint"));
    }

    #[test]
    fn test_inject_breakpoints_inside_multiline_call() {
        // Test breakpoint placed on a line inside a multi-line call expression.
        // The breakpoint is on the argument line, not the start of the expression,
        // but should anchor to the start of the expression.
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

        let mut breakpoints = vec![Breakpoint::new(1, 2, BreakpointState::Unverified)];

        let result = annotate_input(code, location.clone(), Some(&mut breakpoints)).unwrap();
        insta::assert_snapshot!(result);

        // Breakpoint inside a multi-line expression should anchor to expression start
        assert!(!matches!(breakpoints[0].state, BreakpointState::Invalid));
        assert_eq!(
            breakpoints[0].line, 1,
            "Breakpoint should anchor to expression start"
        );
    }
}
