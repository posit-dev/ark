//
// diagnostics.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
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
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_tree_sitter_range_to_lsp_range;
use crate::lsp::indexer;
use crate::lsp::state::WorldState;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnmatchedDelimiterType;

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

    {
        let mut context = DiagnosticContext {
            contents: &doc.contents,
            document_symbols: Vec::new(),
            session_symbols: HashSet::new(),
            workspace_symbols: HashSet::new(),
            installed_packages: HashSet::new(),
            in_formula: false,
            in_call: false,
        };

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
        let result = recurse(root, &mut context, &mut diagnostics);
        if let Err(error) = result {
            log::error!(
                "diagnostics: Error while generating: {error}\n{:#?}",
                error.backtrace()
            );
        }
    }

    diagnostics
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
    // Check for newly-defined variable.
    if let Some(identifier) = identifier {
        // Handle assignments to identifiers, e.g. 'x <- 1'.
        if identifier.is_identifier_or_string() {
            let name = context.contents.node_slice(&identifier)?.to_string();
            let range = identifier.range();
            context.add_defined_variable(name.as_str(), range);
        }

        // Handle assignments to dotty expressions, e.g. '.[a] <- 1'.
        let _: Result<()> = local! {

            // Check that the lhs is a subset call of the form `.[]`.
            identifier.is_subset().into_result()?;
            let function = identifier.child_by_field_name("function").into_result()?;
            function.is_identifier_or_string().into_result()?;
            let value = context.contents.node_slice(&function)?.to_string();
            (value == ".").into_result()?;
            let arguments = identifier.child_by_field_name("arguments").into_result()?;

            // Iterate over each argument, and look for identifiers.
            let mut cursor = arguments.walk();
            for child in arguments.children_by_field_name("argument", &mut cursor) {

                let name = child.child_by_field_name("name");
                let value = child.child_by_field_name("value");
                if let Some(name) = name {
                    let range = name.range();
                    let name = context.contents.node_slice(&name)?.to_string();
                    context.add_defined_variable(name.as_str(), range);
                }
                else if let Some(value) = value {
                    if value.is_identifier_or_string() {
                        let range = value.range();
                        let name = context.contents.node_slice(&value)?.to_string();
                        context.add_defined_variable(name.as_str(), range);
                    }
                }
            }

            Ok(())
        };
    }

    // Recurse into expression for assignment.
    if let Some(expression) = expression {
        recurse(expression, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_namespace(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let lhs = unwrap!(node.child_by_field_name("lhs"), None => {
        return ().ok();
    });

    // Check for a valid package name.
    let package = context.contents.node_slice(&lhs)?.to_string();
    if !context.installed_packages.contains(package.as_str()) {
        let range = lhs.range();
        let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
        let message = format!("package '{}' is not installed", package);
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
    // Check that the opening brace is balanced.
    check_unmatched_opening_brace(node, context, diagnostics)?;

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
    // Check that the opening parenthesis is balanced.
    check_unmatched_opening_paren(node, context, diagnostics)?;

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
        let message = format!("expected at most 1 statement within parentheses, not {n}");
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
    let Some(next) = child.next_sibling() else {
        return ().ok();
    };

    let ok = match next.node_type() {
        NodeType::Comma => true,
        NodeType::Anonymous(kind) if matches!(kind.as_str(), ")") => true,
        NodeType::Comment => true,
        _ => false,
    };

    if ok {
        return ().ok();
    }

    let range = child.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let message = "expected ',' after expression";
    let diagnostic = Diagnostic::new_simple(range, message.into());
    diagnostics.push(diagnostic);

    ().ok()
}

fn check_subset_next_sibling(
    child: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let Some(next) = child.next_sibling() else {
        return ().ok();
    };

    let ok = match next.node_type() {
        NodeType::Comma => true,
        NodeType::Anonymous(kind) if matches!(kind.as_str(), "]" | "]]") => true,
        NodeType::Comment => true,
        _ => false,
    };

    if ok {
        return ().ok();
    }

    let range = child.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let message = "expected ',' after expression";
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

    // Recurse into arguments.
    if let Some(arguments) = node.child_by_field_name("arguments") {
        let mut cursor = arguments.walk();
        let children = arguments.children_by_field_name("argument", &mut cursor);
        for child in children {
            // Warn if the next sibling is neither a comma nor a closing ].
            check_subset_next_sibling(child, context, diagnostics)?;

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

    // Recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        recurse(child, context, diagnostics)?;
    }

    ().ok()
}

fn dispatch(node: Node, context: &mut DiagnosticContext, diagnostics: &mut Vec<Diagnostic>) {
    let result: Result<bool> = local! {
        check_invalid_na_comparison(node, context, diagnostics)?;
        check_symbol_in_scope(node, context, diagnostics)?;
        check_syntax_error(node, context, diagnostics)?;
        check_unclosed_arguments(node, context, diagnostics)?;
        check_unexpected_assignment_in_if_conditional(node, context, diagnostics)?;
        check_unmatched_closing_token(node, context, diagnostics)?;
        true.ok()
    };

    if let Err(error) = result {
        log::error!("{error}");
    }
}

fn check_unmatched_closing_token(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    // These should all be skipped over by function, if, for, while, and repeat
    // handling, so if we ever get here then it means we didn't have an
    // equivalent leading token (or there was some other syntax error that
    // caused the parser to not recognize one of the aforementioned control flow
    // operators, like `repeat { 1 + }`).
    let NodeType::UnmatchedDelimiter(delimiter) = node.node_type() else {
        return false.ok();
    };

    let (token, name) = match delimiter {
        UnmatchedDelimiterType::Brace => ("}", "brace"),
        UnmatchedDelimiterType::Parenthesis => (")", "parenthesis"),
        UnmatchedDelimiterType::Bracket => ("]", "bracket"),
    };

    let range = node.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let message = format!("unmatched closing {name} '{token}'");
    let diagnostic = Diagnostic::new_simple(range, message);
    diagnostics.push(diagnostic);

    true.ok()
}

fn check_unmatched_opening_brace(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    if is_unmatched_block(&node, "{", "}")? {
        let open = node.child(0).unwrap();
        let range = open.range();
        let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
        let message = "unmatched opening brace '{'";
        let diagnostic = Diagnostic::new_simple(range, message.into());
        diagnostics.push(diagnostic);
    }

    true.ok()
}

fn check_unmatched_opening_paren(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    if is_unmatched_block(&node, "(", ")")? {
        let open = node.child(0).unwrap();
        let range = open.range();
        let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
        let message = "unmatched opening parenthesis '('";
        let diagnostic = Diagnostic::new_simple(range, message.into());
        diagnostics.push(diagnostic);
    }

    true.ok()
}

fn is_unmatched_block(node: &Node, open: &str, close: &str) -> Result<bool> {
    let n = node.child_count();

    if n == 0 {
        // Required to have an anonymous `{` or `(` to start the node
        bail!("A `{open}` node must have a minimum size of 1.");
    }

    if n == 1 {
        // No `body` and no closing `token`. Definitely unmatched.
        return true.ok();
    }

    // If `n >= 2`, might be multiple `body`s but still no closing `token`,
    // so we check against the last child.
    let lhs = node.child(1 - 1).unwrap();
    let rhs = node.child(n - 1).unwrap();

    let unmatched = lhs.node_type() == NodeType::Anonymous(open.to_string()) &&
        rhs.node_type() != NodeType::Anonymous(close.to_string());

    unmatched.ok()
}

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
                "NA" => "consider using `is.na()` to check NA values",
                "NaN" => "consider using `is.nan()` to check NaN values",
                "NULL" => "consider using `is.null()` to check NULL values",
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

fn check_syntax_error(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    if !node.is_error() {
        return false.ok();
    }

    let range = node.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let text = context.contents.node_slice(&node)?.to_string();
    let message = format!("Syntax error: unexpected token '{}'", text);
    let diagnostic = Diagnostic::new_simple(range, message);
    diagnostics.push(diagnostic);

    true.ok()
}

fn check_unclosed_arguments(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    let (open, close) = match node.node_type() {
        NodeType::Call => ("(", ")"),
        NodeType::Subset => ("[", "]"),
        NodeType::Subset2 => ("[[", "]]"),
        _ => return false.ok(),
    };

    let arguments = unwrap!(node.child_by_field_name("arguments"), None => {
        return false.ok();
    });

    let n = arguments.child_count();
    if n == 0 {
        return false.ok();
    }

    let lhs = arguments.child(1 - 1).unwrap();
    let rhs = arguments.child(n - 1).unwrap();

    if lhs.node_type() == NodeType::Anonymous(String::from(open)) &&
        rhs.node_type() == NodeType::Anonymous(String::from(close))
    {
        return false.ok();
    }

    let range = lhs.range();
    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
    let message = format!("unmatched opening bracket '{}'", open);
    let diagnostic = Diagnostic::new_simple(range, message);
    diagnostics.push(diagnostic);

    true.ok()
}

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
    let message = "unexpected '='; use '==' to compare values for equality";
    let diagnostic = Diagnostic::new_simple(range, message.into());
    diagnostics.push(diagnostic);

    true.ok()
}

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
    let message = format!("no symbol named '{}' in scope", identifier);
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
    use crate::lsp::diagnostics::is_unmatched_block;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::test::r_test;

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
    fn test_unmatched_braces() {
        let document = Document::new("{", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(is_unmatched_block(&node, "{", "}").unwrap());

        let document = Document::new("{ 1 + 2", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(is_unmatched_block(&node, "{", "}").unwrap());

        let document = Document::new("{}", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(!is_unmatched_block(&node, "{", "}").unwrap());

        let document = Document::new("{ 1 + 2 }", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(!is_unmatched_block(&node, "{", "}").unwrap());
    }

    #[test]
    fn test_unmatched_parentheses() {
        let document = Document::new("(", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(is_unmatched_block(&node, "(", ")").unwrap());

        let document = Document::new("( 1 + 2", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(is_unmatched_block(&node, "(", ")").unwrap());

        let document = Document::new("()", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(!is_unmatched_block(&node, "(", ")").unwrap());

        let document = Document::new("( 1 + 2 )", None);
        let node = document.ast.root_node().named_child(0).unwrap();
        assert!(!is_unmatched_block(&node, "(", ")").unwrap());
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
    fn test_expression_after_call_argument() {
        r_test(|| {
            let text = "match(1, 2 3)";
            let document = Document::new(text, None);

            let diagnostics = generate_diagnostics(document, DEFAULT_STATE.clone());
            assert_eq!(diagnostics.len(), 1);

            let diagnostic = diagnostics.get(0).unwrap();
            assert_eq!(
                diagnostic.message,
                "expected ',' after expression".to_string()
            );
            assert_eq!(diagnostic.range.start, Position::new(0, 9));
            assert_eq!(diagnostic.range.end, Position::new(0, 10));
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
}
