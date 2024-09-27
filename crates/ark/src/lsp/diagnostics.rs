//
// diagnostics.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::bail;
use anyhow::Result;
use harp::utils::is_symbol_valid;
use harp::utils::sym_quote_invalid;
use ropey::Rope;
use stdext::*;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::DiagnosticSeverity;
use tree_sitter::Node;
use tree_sitter::Range;

use crate::lsp::declarations::top_level_declare;
use crate::lsp::diagnostics_syntax::syntax_diagnostics;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_tree_sitter_range_to_lsp_range;
use crate::lsp::indexer;
use crate::lsp::state::WorldState;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::node_has_error_or_missing;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsConfig {
    pub enable: bool,
}

#[derive(Clone)]
pub struct DiagnosticContext<'a> {
    /// The contents of the source document.
    pub contents: &'a Rope,

    /// The symbols currently defined and available in the session.
    pub session_symbols: HashSet<String>,

    /// The symbols used within the document, as a 'stack' of symbols,
    /// mapping symbol names to the locations where they were defined.
    pub document_symbols: Vec<HashMap<String, Range>>,

    /// The symbols defined in the workspace.
    pub workspace_symbols: HashSet<String>,

    // The set of packages that are currently installed.
    pub installed_packages: HashSet<String>,

    // Whether or not we're inside of a formula.
    pub in_formula: bool,

    // Whether or not we're inside of a call's arguments
    pub in_call: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self { enable: true }
    }
}

impl<'a> DiagnosticContext<'a> {
    pub fn new(contents: &'a Rope) -> Self {
        Self {
            contents,
            document_symbols: Vec::new(),
            session_symbols: HashSet::new(),
            workspace_symbols: HashSet::new(),
            installed_packages: HashSet::new(),
            in_formula: false,
            in_call: false,
        }
    }

    pub fn add_defined_variable(&mut self, name: &str, location: Range) {
        let symbols = self.document_symbols.last_mut().unwrap();
        symbols.insert(name.to_string(), location);
    }

    pub fn has_definition(&mut self, name: &str) -> bool {
        // First, check document symbols.
        for symbols in self.document_symbols.iter() {
            if symbols.contains_key(name) {
                return true;
            }
        }

        // Next, check workspace symbols.
        if self.workspace_symbols.contains(name) {
            return true;
        }

        // Finally, check session symbols.
        self.session_symbols.contains(name)
    }
}

pub(crate) fn generate_diagnostics(doc: Document, state: WorldState) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if !state.config.diagnostics.enable {
        return diagnostics;
    }

    // Check that diagnostics are not disabled in top-level declarations for
    // this document
    let decls = top_level_declare(&doc.ast, &doc.contents);
    if !decls.diagnostics {
        return diagnostics;
    }

    let mut context = DiagnosticContext::new(&doc.contents);

    // Add a 'root' context for the document.
    context.document_symbols.push(HashMap::new());

    // Add the current workspace symbols.
    indexer::map(|_path, _symbol, entry| match &entry.data {
        indexer::IndexEntryData::Function { name, arguments: _ } => {
            context.workspace_symbols.insert(name.to_string());
        },
        _ => {},
    });

    for scope in state.console_scopes.iter() {
        for name in scope.iter() {
            if is_symbol_valid(name.as_str()) {
                context.session_symbols.insert(name.clone());
            } else {
                let name = sym_quote_invalid(name.as_str());
                context.session_symbols.insert(name.clone());
            }
        }
    }

    for pkg in state.installed_packages.iter() {
        context.installed_packages.insert(pkg.clone());
    }

    // Start iterating through the nodes.
    let root = doc.ast.root_node();

    // Collect syntax related diagnostics for `ERROR` and `MISSING` nodes
    match syntax_diagnostics(root, &context) {
        Ok(mut syntax_diagnostics) => diagnostics.append(&mut syntax_diagnostics),
        Err(err) => log::error!("Error while generating syntax diagnostics: {err:?}"),
    }

    // Collect semantic related diagnostics
    match semantic_diagnostics(root, &mut context) {
        Ok(mut semantic_diagnostics) => diagnostics.append(&mut semantic_diagnostics),
        Err(err) => log::error!("Error while generating semantic diagnostics: {err:?}"),
    }

    diagnostics
}

fn semantic_diagnostics(
    root: Node,
    context: &mut DiagnosticContext,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    // For semantic diagnostics, we only compute diagnostics for a top level
    // expression if that entire expression's subtree parsed successfully (i.e. if
    // `node.has_error()` returns `false` for that subtree).
    //
    // If a subtree failed to parse successfully, syntax diagnostics will
    // be reported for that subtree instead.
    //
    // This scheme allows us to emit some semantic diagnostics even when
    // we see syntax errors in other parts of the file.
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if node_has_error_or_missing(&child) {
            continue;
        }

        recurse(child, context, &mut diagnostics)?;
    }

    Ok(diagnostics)
}

fn recurse(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    match node.node_type() {
        NodeType::FunctionDefinition => recurse_function(node, context, diagnostics),
        NodeType::ForStatement => recurse_for(node, context, diagnostics),
        NodeType::WhileStatement => recurse_while(node, context, diagnostics),
        NodeType::RepeatStatement => recurse_repeat(node, context, diagnostics),
        NodeType::IfStatement => recurse_if(node, context, diagnostics),
        NodeType::BracedExpression => recurse_braced_expression(node, context, diagnostics),
        NodeType::ParenthesizedExpression => {
            recurse_parenthesized_expression(node, context, diagnostics)
        },
        NodeType::Subset | NodeType::Subset2 => recurse_subset(node, context, diagnostics),
        NodeType::Call => recurse_call(node, context, diagnostics),
        NodeType::UnaryOperator(op) => match op {
            UnaryOperatorType::Tilde => recurse_formula(node, context, diagnostics),
            _ => recurse_default(node, context, diagnostics),
        },
        NodeType::BinaryOperator(op) => match op {
            BinaryOperatorType::Tilde => recurse_formula(node, context, diagnostics),
            BinaryOperatorType::LeftAssignment => {
                recurse_left_assignment(node, context, diagnostics)
            },
            BinaryOperatorType::EqualsAssignment => {
                recurse_equals_assignment(node, context, diagnostics)
            },
            BinaryOperatorType::RightAssignment => {
                recurse_right_assignment(node, context, diagnostics)
            },
            BinaryOperatorType::LeftSuperAssignment => {
                recurse_left_super_assignment(node, context, diagnostics)
            },
            BinaryOperatorType::RightSuperAssignment => {
                recurse_right_super_assignment(node, context, diagnostics)
            },
            _ => recurse_default(node, context, diagnostics),
        },
        NodeType::NamespaceOperator(_) => recurse_namespace(node, context, diagnostics),
        NodeType::Error => bail!("`Error` nodes should have been skipped entirely."),
        _ => recurse_default(node, context, diagnostics),
    }
}

fn recurse_function(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: How should we handle default values for formal arguments to a function?
    // Note that the following is valid R code:
    //
    //    (function(a = b) { b <- 42; a })()
    //
    // So, to accurately diagnose the usage of a formal parameter,
    // we need to see what's in scope at the time when the parameter
    // is first used in the body of the function. (Then, add all the
    // wrinkles related to non-standard evaluation.)

    // Add a new symbols context for this scope.
    let mut context = context.clone();
    context.document_symbols.push(HashMap::new());
    let context = &mut context;

    // Recurse through the arguments, adding their symbols to the `context`
    let parameters = unwrap!(node.child_by_field_name("parameters"), None => {
        bail!("Missing `parameters` field in a `function_definition` node");
    });

    recurse_parameters(parameters, context, diagnostics)?;

    // Recurse through the body, if one exists
    if let Some(body) = node.child_by_field_name("body") {
        recurse(body, context, diagnostics)?;
    }

    Ok(())
}

fn recurse_for(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // First, scan the 'sequence' node.
    let sequence = unwrap!(node.child_by_field_name("sequence"), None => {
        bail!("Missing `sequence` field in a `for` node");
    });

    recurse(sequence, context, diagnostics)?;

    // Now, check for an identifier, and put that in scope.
    let variable = unwrap!(node.child_by_field_name("variable"), None => {
        bail!("Missing `variable` field in a `for` node");
    });

    if variable.is_identifier() {
        let name = context.contents.node_slice(&variable)?.to_string();
        let range = variable.range();
        context.add_defined_variable(name.as_str(), range);
    }

    // Now, scan the body, if it exists
    if let Some(body) = node.child_by_field_name("body") {
        recurse(body, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_if(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // First scan the `condition`.
    let condition = unwrap!(node.child_by_field_name("condition"), None => {
        bail!("Missing `condition` field in an `if` node.");
    });

    recurse(condition, context, diagnostics)?;

    // Now, scan the `consequence`.
    let consequence = unwrap!(node.child_by_field_name("consequence"), None => {
        bail!("Missing `consequence` field in an `if` node.");
    });

    recurse(consequence, context, diagnostics)?;

    // And finally the optional `alternative`
    if let Some(alternative) = node.child_by_field_name("alternative") {
        recurse(alternative, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_while(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // First scan the `condition`.
    let condition = unwrap!(node.child_by_field_name("condition"), None => {
        bail!("Missing `condition` field in a `while` node.");
    });

    recurse(condition, context, diagnostics)?;

    // Now, scan the `body`, if it exists.
    if let Some(body) = node.child_by_field_name("body") {
        recurse(body, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_repeat(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Only thing to scan is the `body`, if it exists
    if let Some(body) = node.child_by_field_name("body") {
        recurse(body, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_formula(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: Are there any sensible diagnostics we can do in a formula?
    // Beyond just checking for syntax errors, or things of that form?
    let mut context = context.clone();
    context.in_formula = true;
    let context = &mut context;

    if let Some(lhs) = node.child_by_field_name("lhs") {
        recurse(lhs, context, diagnostics)?;
    }
    if let Some(rhs) = node.child_by_field_name("rhs") {
        recurse(rhs, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_left_super_assignment(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let identifier = node.child_by_field_name("lhs");
    let expression = node.child_by_field_name("rhs");
    recurse_super_assignment(identifier, expression, context, diagnostics)
}

fn recurse_right_super_assignment(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let identifier = node.child_by_field_name("rhs");
    let expression = node.child_by_field_name("lhs");
    recurse_super_assignment(identifier, expression, context, diagnostics)
}

fn recurse_super_assignment(
    identifier: Option<Node>,
    expression: Option<Node>,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: Check for a target within a parent scope.
    // We could probably add some more advanced diagnostics here, but for
    // now we want to make sure that the `identifier` isn't hit with a "symbol
    // not in scope" diagnostic, so we add it to the `document_symbols` map.
    recurse_assignment(identifier, expression, context, diagnostics)
}

fn recurse_left_assignment(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let identifier = node.child_by_field_name("lhs");
    let expression = node.child_by_field_name("rhs");
    recurse_assignment(identifier, expression, context, diagnostics)
}

fn recurse_equals_assignment(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let identifier = node.child_by_field_name("lhs");
    let expression = node.child_by_field_name("rhs");
    recurse_assignment(identifier, expression, context, diagnostics)
}

fn recurse_right_assignment(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let identifier = node.child_by_field_name("rhs");
    let expression = node.child_by_field_name("lhs");
    recurse_assignment(identifier, expression, context, diagnostics)
}

fn recurse_assignment(
    identifier: Option<Node>,
    expression: Option<Node>,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    if let Some(identifier) = identifier {
        // Check for newly-defined variable
        handle_assignment_variable(&identifier, context)?;

        // Check for dotty assignment
        handle_assignment_dotty(&identifier, context)?;
    }

    // Recurse into expression for assignment.
    if let Some(expression) = expression {
        recurse(expression, context, diagnostics)?;
    }

    ().ok()
}

fn handle_assignment_variable(
    identifier: &Node,
    context: &mut DiagnosticContext,
) -> anyhow::Result<()> {
    if !identifier.is_identifier_or_string() {
        return Ok(());
    }

    let name = context.contents.node_slice(&identifier)?.to_string();
    let range = identifier.range();
    context.add_defined_variable(name.as_str(), range);

    Ok(())
}

/// Support for dotty assignment
///
/// Comes in a few forms
/// - `.[x, y] <- list(1, 2)`
/// - `.[one, .., five] <- list(1, 2, 3, 4, 5)`
/// - `.[foo = y] <- list(x = 1, y = 2, z = 3)`
/// - `.[x, .[y, .[z]]] <- list(1, list(2, list(3)))`
///
/// https://cran.r-project.org/web/packages/dotty/index.html
fn handle_assignment_dotty(
    identifier: &Node,
    context: &mut DiagnosticContext,
) -> anyhow::Result<()> {
    if !identifier.is_subset() {
        // Not a subset call of the form `.[]`
        return Ok(());
    };

    let Some(dot) = identifier.child_by_field_name("function") else {
        return Ok(());
    };
    if !dot.is_identifier() {
        return Ok(());
    };

    let dot = context.contents.node_slice(&dot)?;
    if dot != "." {
        return Ok(());
    };

    // Make sure we're not being invoked within a magrittr pipe, since
    // '.' has special semantics in that scope.
    if node_is_within_magrittr_pipe(identifier, context)? {
        return Ok(());
    }

    // Iterate over each argument, and look for identifiers.
    let Some(arguments) = identifier.child_by_field_name("arguments") else {
        return Ok(());
    };

    let mut cursor = arguments.walk();

    for child in arguments.children_by_field_name("argument", &mut cursor) {
        // i.e. `.[foo = x] <- lst`
        // If we find a name, we are done. The above example means "extract x from lst as foo",
        // so we don't want to define a variable for `x` there.
        if let Some(name) = child.child_by_field_name("name") {
            let range = name.range();
            let name = context.contents.node_slice(&name)?.to_string();
            context.add_defined_variable(name.as_str(), range);
            continue;
        };

        let Some(value) = child.child_by_field_name("value") else {
            continue;
        };

        // i.e. `.[x, y]` where `value` is just a name that dotty assigns to
        if value.is_identifier() {
            let range = value.range();
            let name = context.contents.node_slice(&value)?.to_string();
            context.add_defined_variable(name.as_str(), range);
            continue;
        }

        // i.e. `.[x, .[y]]` where unpacking is recursive
        if value.is_subset() {
            handle_assignment_dotty(&value, context)?;
            continue;
        }
    }

    Ok(())
}

fn node_is_within_magrittr_pipe(node: &Node, context: &DiagnosticContext) -> anyhow::Result<bool> {
    // Start search with the parent so we don't return `true` if we are already on the `%>%`
    let Some(node) = node.parent() else {
        return Ok(false);
    };

    match node_find_magrittr_pipe(&node, context)? {
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

fn node_find_magrittr_pipe<'tree>(
    node: &Node<'tree>,
    context: &DiagnosticContext,
) -> anyhow::Result<Option<Node<'tree>>> {
    if node.is_magrittr_pipe_operator(&context.contents)? {
        // Found one!
        return Ok(Some(*node));
    }

    if node.is_braced_expression() {
        // Stop recursion if we hit a brace
        return Ok(None);
    }

    let Some(node) = node.parent() else {
        // Stop recursion if no parent
        return Ok(None);
    };

    node_find_magrittr_pipe(&node, context)
}

fn recurse_namespace(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // This is really a syntax issue, but the RHS is optional in the grammar,
    // so `dplyr::` is technically allowed and we have to check for this in
    // the semantic path instead.
    crate::lsp::diagnostics_syntax::diagnose_missing_namespace_operator(
        node,
        context,
        diagnostics,
    )?;

    let lhs = unwrap!(node.child_by_field_name("lhs"), None => {
        return ().ok();
    });

    // Check for a valid package name.
    let package = context.contents.node_slice(&lhs)?.to_string();
    if !context.installed_packages.contains(package.as_str()) {
        let range = lhs.range();
        let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
        let message = format!("Package '{}' is not installed.", package);
        let diagnostic = Diagnostic::new_simple(range, message);
        diagnostics.push(diagnostic);
    }

    // Check for a symbol in this namespace.
    let rhs = unwrap!(node.child_by_field_name("rhs"), None => {
        return ().ok();
    });

    if !rhs.is_identifier_or_string() {
        return ().ok();
    }

    // TODO: Check if this variable is defined in the requested namespace.
    ().ok()
}

fn recurse_parameters(
    node: Node,
    context: &mut DiagnosticContext,
    _diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: Should we do anything with default values? i.e. `function(x = 4)`?
    // They are marked with a field name of `"default"`.
    let mut cursor = node.walk();

    for child in node.children_by_field_name("parameter", &mut cursor) {
        let name = unwrap!(child.child_by_field_name("name"), None => {
            bail!("Missing a `name` field in a `parameter` node.");
        });

        let symbol = unwrap!(context.contents.node_slice(&name), Err(error) => {
            bail!("Failed to convert `name` node to a string due to: {error}");
        });
        let symbol = symbol.to_string();

        let location = name.range();

        context.add_defined_variable(symbol.as_str(), location);
    }

    ().ok()
}

fn recurse_braced_expression(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Recurse into body statements.
    let mut cursor = node.walk();

    for child in node.children_by_field_name("body", &mut cursor) {
        recurse(child, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_parenthesized_expression(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let mut n = 0;
    let mut cursor = node.walk();

    for child in node.children_by_field_name("body", &mut cursor) {
        recurse(child, context, diagnostics)?;
        n = n + 1;
    }

    if n > 1 {
        // The tree-sitter grammar allows multiple `body` statements, but we warn
        // the user about this as it is not allowed by the R parser.
        let range = node.range();
        let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
        let message = format!("Expected at most 1 statement within parentheses, found {n}.");
        let diagnostic = Diagnostic::new_simple(range, message);
        diagnostics.push(diagnostic);
    }

    ().ok()
}

fn check_call_next_sibling(
    child: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    check_call_like_next_sibling(child, &NodeType::Call, context, diagnostics)
}

fn check_subset_next_sibling(
    child: Node,
    subset_type: &NodeType,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    check_call_like_next_sibling(child, &subset_type, context, diagnostics)
}

// TODO: This should be a syntax check, as the grammar should not allow
// two `Argument` nodes side by side
fn check_call_like_next_sibling(
    child: Node,
    parent_type: &NodeType,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let Some(next) = child.next_sibling() else {
        return ().ok();
    };

    let close = match parent_type {
        NodeType::Call => ")",
        NodeType::Subset => "]",
        NodeType::Subset2 => "]]",
        _ => bail!("Parent must be a call, subset, or subset2 node."),
    };

    let ok = match next.node_type() {
        NodeType::Comma => true,
        NodeType::Anonymous(kind) if kind.as_str() == close => true,
        NodeType::Comment => true,
        // Should be handled elsewhere
        NodeType::Error => true,
        _ => false,
    };

    if ok {
        return ().ok();
    }

    // Children can be arbitrarily large, so report the issue between the end of `child`
    // and the start of `next` (it's not really the child's fault anyways,
    // it's the fault of the thing right after it).
    let start_byte = child.end_byte();
    let start_point = child.end_position();
    let end_byte = next.start_byte();
    let end_point = next.start_position();

    let range = Range {
        start_byte,
        start_point,
        end_byte,
        end_point,
    };

    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let message = "Expected ',' between expressions.";
    let diagnostic = Diagnostic::new_simple(range, message.into());
    diagnostics.push(diagnostic);

    ().ok()
}

// Default recursion for arguments of a function call
fn recurse_call_arguments_default(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: Can we better handle NSE in things like `quote()` and
    // `dplyr::mutate()` so we don't have to turn off certain diagnostics when
    // we are inside a call's arguments?
    let mut context = context.clone();
    context.in_call = true;
    let context = &mut context;

    // Recurse into arguments.
    if let Some(arguments) = node.child_by_field_name("arguments") {
        let mut cursor = arguments.walk();
        let children = arguments.children_by_field_name("argument", &mut cursor);
        for child in children {
            // Warn if the next sibling is neither a comma nor a closing delimiter.
            check_call_next_sibling(child, context, diagnostics)?;

            // Recurse into values.
            if let Some(value) = child.child_by_field_name("value") {
                recurse(value, context, diagnostics)?;
            }
        }
    }

    ().ok()
}

fn recurse_call(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Run diagnostics on the call itself
    dispatch(node, context, diagnostics);

    // Recurse into the callee.
    let callee = node.child(0).into_result()?;
    recurse(callee, context, diagnostics)?;

    // dispatch based on the function
    //
    // TODO: Handle certain 'scope-generating' function calls, e.g.
    // things like 'local({ ... })'.
    let fun = context.contents.node_slice(&callee)?.to_string();
    let fun = fun.as_str();

    match fun {
        // default case: recurse into each argument
        _ => recurse_call_arguments_default(node, context, diagnostics)?,
    };

    ().ok()
}

fn recurse_subset(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Run diagnostics on the call.
    dispatch(node, context, diagnostics);

    // Recurse into the callee.
    if let Some(callee) = node.child(0) {
        recurse(callee, context, diagnostics)?;
    }

    let subset_type = node.node_type();

    // Recurse into arguments.
    if let Some(arguments) = node.child_by_field_name("arguments") {
        let mut cursor = arguments.walk();
        let children = arguments.children_by_field_name("argument", &mut cursor);
        for child in children {
            // Warn if the next sibling is neither a comma nor a closing ].
            check_subset_next_sibling(child, &subset_type, context, diagnostics)?;

            // Recurse into values.
            if let Some(value) = child.child_by_field_name("value") {
                recurse(value, context, diagnostics)?;
            }
        }
    }

    ().ok()
}

fn recurse_default(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Apply diagnostic functions to node.
    dispatch(node, context, diagnostics);
    recurse_children(node, context, diagnostics)
}

fn recurse_children(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        recurse(child, context, diagnostics)?;
    }

    Ok(())
}

fn dispatch(node: Node, context: &mut DiagnosticContext, diagnostics: &mut Vec<Diagnostic>) {
    let result: Result<bool> = local! {
        check_invalid_na_comparison(node, context, diagnostics)?;
        check_symbol_in_scope(node, context, diagnostics)?;
        check_unexpected_assignment_in_if_conditional(node, context, diagnostics)?;
        true.ok()
    };

    if let Err(error) = result {
        log::error!("{error}");
    }
}

// TODO: Move this to `recurse_binary_equal()` and get it out of `dispatch()`
fn check_invalid_na_comparison(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    let n = node.child_count();
    if n == 0 {
        return false.ok();
    }

    if node.node_type() != NodeType::BinaryOperator(BinaryOperatorType::Equal) {
        return false.ok();
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let contents = context.contents.node_slice(&child)?.to_string();
        let contents = contents.as_str();

        if matches!(contents, "NA" | "NaN" | "NULL") {
            let message = match contents {
                "NA" => "Consider using `is.na()` to check `NA` values",
                "NaN" => "Consider using `is.nan()` to check `NaN` values.",
                "NULL" => "Consider using `is.null()` to check `NULL` values.",
                _ => continue,
            };
            let range = child.range();
            let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
            let mut diagnostic = Diagnostic::new_simple(range, message.into());
            diagnostic.severity = Some(DiagnosticSeverity::INFORMATION);
            diagnostics.push(diagnostic);
        }
    }

    true.ok()
}

// TODO: Move this to `recurse_if()` and get it out of `dispatch()`
fn check_unexpected_assignment_in_if_conditional(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    let n = node.child_count();
    if n == 0 {
        return false.ok();
    }

    if node.node_type() != NodeType::IfStatement {
        return false.ok();
    }

    let condition = unwrap!(node.child_by_field_name("condition"), None => {
        return false.ok();
    });

    if condition.node_type() != NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment) {
        return false.ok();
    }

    let range = condition.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let message = "Unexpected '='; use '==' to compare values for equality.";
    let diagnostic = Diagnostic::new_simple(range, message.into());
    diagnostics.push(diagnostic);

    true.ok()
}

// TODO: Move this to `recurse_identifier()` and get it out of `dispatch()`
fn check_symbol_in_scope(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    // Skip if we're in a formula.
    if context.in_formula {
        return false.ok();
    }

    // Skip if we're working on the arguments of a call
    if context.in_call {
        return false.ok();
    }

    // Skip if this isn't an identifier.
    if !node.is_identifier() {
        return false.ok();
    }

    // Skip if this identifier belongs to a '$' or `@` node.
    if let Some(parent) = node.parent() {
        if matches!(parent.node_type(), NodeType::ExtractOperator(_)) {
            if let Some(rhs) = parent.child_by_field_name("rhs") {
                if rhs == node {
                    return false.ok();
                }
            }
        }
    }

    // Skip if a symbol with this name is in scope.
    let name = context.contents.node_slice(&node)?.to_string();
    if context.has_definition(name.as_str()) {
        return false.ok();
    }

    // No symbol in scope; provide a diagnostic.
    let range = node.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let identifier = context.contents.node_slice(&node)?.to_string();
    let message = format!("No symbol named '{}' in scope.", identifier);
    let mut diagnostic = Diagnostic::new_simple(range, message);
    diagnostic.severity = Some(DiagnosticSeverity::WARNING);
    diagnostics.push(diagnostic);

    true.ok()
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;
    use once_cell::sync::Lazy;
    use tower_lsp::lsp_types::Position;

    use crate::interface::console_inputs;
    use crate::lsp::diagnostics::generate_diagnostics;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::fixtures::r_test;

    // Default state that includes installed packages and default scopes.
    static DEFAULT_STATE: Lazy<WorldState> = Lazy::new(|| current_state());

    fn current_state() -> WorldState {
        let inputs = console_inputs().unwrap();

        WorldState {
            console_scopes: inputs.console_scopes,
            installed_packages: inputs.installed_packages,
            ..Default::default()
        }
    }

    #[test]
    fn test_mixed_syntax_and_semantic_diagnostics() {
        r_test(|| {
            // - `foo` is an unknown symbol
            // - `}` is a syntax error, but localized to that line
            let text = "
foo

1 }";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 2);

            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
            assert_eq!(diagnostic.range.start, Position::new(3, 2));
            assert_eq!(diagnostic.range.end, Position::new(3, 3));

            let diagnostic = diagnostics.get(1).unwrap();
            insta::assert_snapshot!(diagnostic.message);
            assert_eq!(diagnostic.range.start, Position::new(1, 0));
            assert_eq!(diagnostic.range.end, Position::new(1, 3));
        })
    }

    #[test]
    fn test_comment_after_call_argument() {
        r_test(|| {
            let text = "
            match(
                1,
                2 # hi there
            )";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());
            assert!(diagnostics.is_empty());
        })
    }

    #[test]
    fn test_missing_namespace_rhs() {
        r_test(|| {
            let text = "base::";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);
            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_expression_after_call_argument() {
        r_test(|| {
            let text = "match(1, 2 3)";
            let document = Document::new(text, None);

            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            // Diagnostic highlights between the `2` and `3`
            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
            assert_eq!(diagnostic.range.start, Position::new(0, 10));
            assert_eq!(diagnostic.range.end, Position::new(0, 11));
        })
    }

    #[test]
    fn test_no_diagnostic_for_dot_dot_i() {
        r_test(|| {
            let text = "..1 + ..2 + 3";
            let document = Document::new(text, None);

            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());

            assert!(diagnostics.is_empty());
        })
    }

    #[test]
    fn test_no_diagnostic_for_rhs_of_extractor() {
        r_test(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Put the LHS in scope
            harp::parse_eval("x <- NULL", options.clone()).unwrap();
            let state = current_state();

            let text = "x$foo";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document.clone(), state.clone());
            assert!(diagnostics.is_empty());

            let text = "x@foo";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document.clone(), state.clone());
            assert!(diagnostics.is_empty());

            // Clean up
            harp::parse_eval("remove(x)", options.clone()).unwrap();
        })
    }

    #[test]
    fn test_no_diagnostic_for_assignment_bindings() {
        r_test(|| {
            let text = "
                x <- 1
                2 -> y
                z = 3
                y + x + z
            ";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert!(diagnostics.is_empty());
        })
    }

    #[test]
    fn test_no_diagnostic_for_super_assignment_bindings() {
        r_test(|| {
            let text = "
                x <<- 1
                2 ->> y
                y + x
            ";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert!(diagnostics.is_empty());
        })
    }

    #[test]
    fn test_symbol_not_in_scope_diagnostic_is_ordering_dependent() {
        r_test(|| {
            let text = "
                x + 1
                x <- 1
                x + 1
            ";
            let document = Document::new(text, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            // Only marks the `x` before the `x <- 1`
            let diagnostic = diagnostics.get(0).unwrap();
            assert_eq!(diagnostic.range.start.line, 1)
        })
    }

    #[test]
    fn test_no_diagnostic_formula() {
        r_test(|| {
            let text = "
                foo ~ bar
                ~foo
                identity(foo ~ bar)
                identity(~foo)
            ";
            let document = Document::new(text, None);
            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());
            assert!(diagnostics.is_empty());
        })
    }

    #[test]
    fn test_dotty_assignment_basic() {
        r_test(|| {
            let code = "
                .[apple, banana] <- c(1, 2)
                apple
                banana
                cherry
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_dotty_right_assignment_basic() {
        r_test(|| {
            let code = "
                c(1, 2) -> .[apple, banana]
                apple
                banana
                cherry
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_dotty_assignment_named() {
        r_test(|| {
            // `x` should not be defined
            let code = "
                .[apple = x, banana] <- list(w = 1, x = 2, y = 3, z = 4)
                apple
                banana
                x
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_dotty_assignment_recursive() {
        r_test(|| {
            let code = "
                .[apple, .[banana]] <- list(1, list(2))
                apple
                banana
                cherry
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_dotty_assignment_within_magrittr_pipe_braced_expr() {
        r_test(|| {
            let code = "
                mtcars %>% list({ .[apple] <- 1; apple })
                apple
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            assert_eq!(diagnostic.range.start.line, 2);
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_dotty_assignment_within_native_pipe_braced_expr() {
        // TODO: `apple` should be defined in the global env and there should not be a diagnostic here
        r_test(|| {
            let code = "
                mtcars |> list({ .[apple] <- 1; apple })
                apple
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            assert_eq!(diagnostic.range.start.line, 2);
            insta::assert_snapshot!(diagnostic.message);
        })
    }

    #[test]
    fn test_assignment_within_function_arguments() {
        // TODO: `x` should be defined in the global env and there should not be a diagnostic here
        r_test(|| {
            let code = "
                list({ x <- 1 })
                x
            ";

            let document = Document::new(code, None);

            let diagnostics = generate_diagnostics(document.clone(), DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            assert_eq!(diagnostic.range.start.line, 2);
            insta::assert_snapshot!(diagnostic.message);
        })
    }
}
