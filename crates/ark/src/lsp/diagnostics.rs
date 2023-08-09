//
// diagnostics.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::collections::HashSet;
use std::os::raw::c_void;
use std::sync::atomic::AtomicI64;
use std::time::Duration;

use anyhow::Result;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::external_ptr::ExternalPointer;
use harp::object::RObject;
use harp::protect::RProtect;
use harp::r_lock;
use harp::r_symbol;
use harp::utils::r_is_null;
use harp::utils::r_symbol_quote_invalid;
use harp::utils::r_symbol_valid;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::*;
use stdext::*;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::DiagnosticSeverity;
use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::lsp::backend::Backend;
use crate::lsp::indexer;
use crate::Range;

static DIAGNOSTICS_VERSION: AtomicI64 = AtomicI64::new(0);

#[derive(Clone)]
pub struct DiagnosticContext<'a> {
    /// The contents of the source document.
    pub source: &'a str,

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

pub async fn enqueue_diagnostics(backend: Backend, uri: Url) {
    // Bump to the version associated with this diagnostics call.
    let version = DIAGNOSTICS_VERSION.load(std::sync::atomic::Ordering::Acquire) + 1;
    // log::trace!("[diagnostics({version})] Spawning task to enqueue diagnostics.");

    // Store the version we're planning to apply diagnostics for.
    DIAGNOSTICS_VERSION.store(version, std::sync::atomic::Ordering::Release);

    // Spawn a task to enqueue diagnostics.
    tokio::spawn(async move {
        // Wait some amount of time. Note that the document version is updated on
        // every document change, so if the document changes while this task is waiting,
        // we'll see that the global DIAGNOSTICS_VERSION is now out-of-sync with the version
        // associated with this task, and toss it away.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let current_version = DIAGNOSTICS_VERSION.load(std::sync::atomic::Ordering::Acquire);
        if version != current_version {
            // log::trace!("[diagnostics({version})] Aborting diagnostics in favor of version {current_version}.");
            return;
        }

        // Okay, it's our chance to provide diagnostics.
        // log::trace!("[diagnostics({version})] Generating diagnostics.");
        enqueue_diagnostics_impl(backend, uri).await;
    });
}

async fn enqueue_diagnostics_impl(backend: Backend, uri: Url) {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    {
        // get reference to document
        let doc = unwrap!(backend.documents.get_mut(&uri), None => {
            log::error!("diagnostics: no document associated with uri {} available", uri);
            return;
        });

        let source = doc.contents.to_string();
        let mut context = DiagnosticContext {
            source: source.as_str(),
            document_symbols: Vec::new(),
            session_symbols: HashSet::new(),
            workspace_symbols: HashSet::new(),
            installed_packages: HashSet::new(),
            in_formula: false,
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

        r_lock! {
            // Get the set of symbols currently in scope.
            let mut envir = R_GlobalEnv;
            while envir != R_EmptyEnv {

                // List symbol names in this environment.
                let mut protect = RProtect::new();
                let objects = protect.add(R_lsInternal(envir, 1));

                // Ensure that non-syntactic names are quoted.
                let vector = CharacterVector::new(objects).unwrap();
                for name in vector.iter() {
                    if let Some(name) = name {
                        if r_symbol_valid(name.as_str()) {
                            context.session_symbols.insert(name);
                        } else {
                            let name = r_symbol_quote_invalid(name.as_str());
                            context.session_symbols.insert(name);
                        }
                    }
                }

                envir = ENCLOS(envir);
            }

            // Get the set of installed packages.
            let packages = RFunction::new("base", ".packages")
                .param("all.available", true)
                .call()
                .unwrap();

            let vector = CharacterVector::new(packages).unwrap();
            for name in vector.iter() {
                if let Some(name) = name {
                    context.installed_packages.insert(name);
                }
            }
        }

        // Start iterating through the nodes.
        let root = doc.ast.root_node();
        let result = recurse(root, &mut context, &mut diagnostics);
        if let Err(error) = result {
            log::error!("{:#?}", error.backtrace());
        }
    }

    backend
        .client
        .publish_diagnostics(uri, diagnostics, None)
        .await;
}

fn recurse(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    match node.kind() {
        "function" => recurse_function(node, context, diagnostics),
        "for" => recurse_for(node, context, diagnostics),
        "~" => recurse_formula(node, context, diagnostics),
        "<<-" => recurse_superassignment(node, context, diagnostics),
        "<-" => recurse_assignment(node, context, diagnostics),
        "::" | ":::" => recurse_namespace(node, context, diagnostics),
        "{" => recurse_block(node, context, diagnostics),
        "(" => recurse_paren(node, context, diagnostics),
        "[" | "[[" => recurse_subset(node, context, diagnostics),
        "call" => recurse_call(node, context, diagnostics),
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

    // Recurse through the children of this node.
    if let Some(parameters) = node.child_by_field_name("parameters") {
        recurse_parameters(parameters, context, diagnostics)?;
    }

    // Recurse over children.
    let mut cursor = node.walk();
    let children = node.children(&mut cursor);
    for child in children {
        recurse(child, context, diagnostics)?;
    }

    Ok(())
}

fn recurse_for(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // First, scan the 'sequence' node.
    if let Some(sequence) = node.child_by_field_name("sequence") {
        recurse(sequence, context, diagnostics)?;
    }

    // Now, check for an identifier, and put that in scope.
    if let Some(identifier) = node.child_by_field_name("variable") {
        if identifier.kind() == "identifier" {
            let name = identifier.utf8_text(context.source.as_bytes())?;
            let range: Range = identifier.range().into();
            context.add_defined_variable(name.into(), range.into());
        }
    }

    // Now, scan the body.
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

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        recurse(child, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_superassignment(
    _node: Node,
    _context: &mut DiagnosticContext,
    _diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: Check for a target within a parent scope.
    ().ok()
}

fn recurse_assignment(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Check for newly-defined variable.
    if let Some(lhs) = node.child_by_field_name("lhs") {
        if matches!(lhs.kind(), "identifier" | "string") {
            let name = lhs.utf8_text(context.source.as_bytes())?;
            let range: Range = lhs.range().into();
            context.add_defined_variable(name, range.into());
        }
    }

    // Recurse into expression for assignment.
    if let Some(rhs) = node.child_by_field_name("rhs") {
        recurse(rhs, context, diagnostics)?;
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
    let package = lhs.utf8_text(context.source.as_bytes())?;
    if !context.installed_packages.contains(package) {
        let range: Range = lhs.range().into();
        let message = format!("package '{}' is not installed", package);
        let diagnostic = Diagnostic::new_simple(range.into(), message);
        diagnostics.push(diagnostic);
    }

    // Check for a symbol in this namespace.
    let rhs = unwrap!(node.child_by_field_name("rhs"), None => {
        return ().ok();
    });

    if !matches!(rhs.kind(), "identifier" | "string") {
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
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(name) = child.child_by_field_name("name") {
            let symbol = name.utf8_text(context.source.as_bytes())?;
            let location: Range = name.range().into();
            context.add_defined_variable(symbol, location.into());
        }
    }
    ().ok()
}

fn recurse_block(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Check that the opening brace is balanced.
    check_unmatched_opening_brace(node, context, diagnostics)?;

    // Recurse into body statements.
    let mut cursor = node.walk();
    let children = node.children(&mut cursor);
    for child in children {
        recurse(child, context, diagnostics)?;
    }

    ().ok()
}

fn recurse_paren(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // TODO: Warn if multiple 'body' children? The tree-sitter
    // grammar allows it, but we should warn when we encounter
    // more than one 'body' statement in parentheses, as that is
    // not permitted by the R parser.
    let mut cursor = node.walk();
    let children = node.children(&mut cursor);
    for child in children {
        recurse(child, context, diagnostics)?;
    }

    ().ok()
}

fn check_call_next_sibling(
    child: Node,
    _context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    if let Some(next) = child.next_sibling() {
        if !matches!(next.kind(), "comma" | ")") {
            let range: Range = child.range().into();
            let message = "expected ',' after expression";
            let diagnostic = Diagnostic::new_simple(range.into(), message.into());
            diagnostics.push(diagnostic);
        }
    }

    ().ok()
}

fn check_subset_next_sibling(
    child: Node,
    _context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    if let Some(next) = child.next_sibling() {
        if !matches!(next.kind(), "comma" | "]" | "]]") {
            let range: Range = child.range().into();
            let message = "expected ',' after expression";
            let diagnostic = Diagnostic::new_simple(range.into(), message.into());
            diagnostics.push(diagnostic);
        }
    }

    ().ok()
}

// Default recursion for arguments of a function call
fn recurse_call_arguments_default(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
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

struct TreeSitterCall<'a> {
    // A call of the form <fun>(list(0L, <ptr>), foo = list(1L, <ptr>))
    call: RObject,

    // holding on to each "value" Node for as long as call lives
    _value_nodes: Vec<Node<'a>>,
}

impl<'a> From<&TreeSitterCall<'a>> for RObject {
    fn from(value: &TreeSitterCall<'a>) -> Self {
        value.call.clone()
    }
}

impl<'a> TreeSitterCall<'a> {
    pub unsafe fn new(
        node: Node<'a>,
        function: &str,
        context: &mut DiagnosticContext,
    ) -> Result<Self> {
        // start with a call to the function: <fun>()
        let sym = r_symbol!(function);
        let call = RObject::new(Rf_lang1(sym));

        // then augment it with arguments
        let mut tail = *call;

        // values are stored in the call as external pointers
        // to Node that are kept alive in this vector and
        // in TreeSitterCall::_values
        let mut _value_nodes = vec![];

        if let Some(arguments) = node.child_by_field_name("arguments") {
            let mut cursor = arguments.walk();
            let children = arguments.children_by_field_name("argument", &mut cursor);
            let mut i = 0;
            for child in children {
                let arg_list = RObject::from(Rf_allocVector(VECSXP, 2));

                // set the argument to a list<2>, with its first element: a scalar integer
                // that corresponds to its O-based position. The position is used below to
                // map back to the Node
                SET_VECTOR_ELT(*arg_list, 0, Rf_ScalarInteger(i as i32));

                // Set the second element of the list to an external pointer
                // to the child node.
                if let Some(value) = child.child_by_field_name("value") {
                    _value_nodes.push(value);
                    let value_node_ptr = ExternalPointer::new(_value_nodes.last().into_result()?);
                    SET_VECTOR_ELT(*arg_list, 1, *value_node_ptr.pointer);
                }

                SETCDR(tail, Rf_cons(*arg_list, R_NilValue));
                tail = CDR(tail);

                // potentially add the argument name
                if let Some(name) = child.child_by_field_name("name") {
                    let name = name.utf8_text(context.source.as_bytes())?;
                    let sym_name = r_symbol!(name);
                    SET_TAG(tail, sym_name);
                }

                i = i + 1;
            }
        }

        Ok(Self { call, _value_nodes })
    }
}

fn recurse_call_arguments_custom(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    diagnostic_function: &str,
) -> Result<()> {
    r_lock! {
        // Build a call that mixes treesitter nodes (as external pointers)
        // library(foo, pos = 2 + 2)
        //    ->
        // library([0, <node0>], pos = [1, <node1>])
        // where:
        //   - node0 is an external pointer to a treesitter Node for the identifier `foo`
        //   - node1 is an external pointer to a treesitter Node for the call `2 + 2`
        //
        // The TreeSitterCall object holds on to the nodes, so that they can be
        // safely passed down to the R side as external pointers
        let call = TreeSitterCall::new(node, function, context)?;

        let custom_diagnostics = RFunction::from(diagnostic_function)
            .add(&call)
            .add(ExternalPointer::new(&context.source))
            .call()?;

        if !r_is_null(*custom_diagnostics) {
            let n = XLENGTH(*custom_diagnostics);
            for i in 0..n {
                // diag is a list with:
                //   - The kind of diagnostic: skip, default, simple
                //   - The node external pointer, i.e. the ones made in TreeSitterCall::new
                //   - The message, when kind is "simple"
                let diag = VECTOR_ELT(*custom_diagnostics, i);

                let kind: String = RObject::view(VECTOR_ELT(diag, 0)).try_into()?;

                if kind == "skip" {
                    // skip the diagnostic entirely, e.g.
                    // library(foo)
                    //         ^^^
                    continue;
                }

                let ptr = VECTOR_ELT(diag, 1);
                let value = *(R_ExternalPtrAddr(ptr) as *const c_void as *const Node);

                if kind == "default" {
                    // the R side gives up, so proceed as normal, e.g.
                    // library(foo, pos = ...)
                    //                    ^^^
                    recurse(value, context, diagnostics)?;
                } else if kind == "simple" {
                    // Simple diagnostic from R, e.g.
                    // library("ggplot3")
                    //          ^^^^^^^   Package 'ggplot3' is not installed
                    let message: String = RObject::view(VECTOR_ELT(diag, 2)).try_into()?;
                    let range: Range = value.range().into();
                    let diagnostic = Diagnostic::new_simple(range.into(), message.into());
                    diagnostics.push(diagnostic);
                }
            }
        }

        ().ok()
    }
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
    let fun = callee.utf8_text(context.source.as_bytes())?;
    match fun {
        // TODO: there should be some sort of registration mechanism so
        //       that functions can declare that they know how to generate
        //       diagnostics, i.e. ggplot2::aes() would skip diagnostics

        // special case to deal with library() and require() nse
        "library" => recurse_call_arguments_custom(
            node,
            context,
            diagnostics,
            "library",
            ".ps.diagnostics.custom.library",
        )?,
        "require" => recurse_call_arguments_custom(
            node,
            context,
            diagnostics,
            "require",
            ".ps.diagnostics.custom.require",
        )?,

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
        check_unmatched_closing_bracket(node, context, diagnostics)?;
        true.ok()
    };

    if let Err(error) = result {
        log::error!("{}", error);
    }
}

fn check_unmatched_closing_bracket(
    node: Node,
    _context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    // TODO: Can we infer these node kinds in a better way?
    if !matches!(node.kind_id(), 72 | 73 | 74) {
        return false.ok();
    }

    let bracket = match node.kind() {
        "}" => "brace",
        ")" => "paren",
        "]" => "bracket",
        _ => "bracket",
    };

    let range: Range = node.range().into();
    let message = format!("unmatched closing {} '{}'", bracket, node.kind());
    let diagnostic = Diagnostic::new_simple(range.into(), message.into());
    diagnostics.push(diagnostic);

    true.ok()
}

fn check_unmatched_opening_brace(
    node: Node,
    _context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    let n = node.child_count();
    if n == 0 {
        return false.ok();
    }

    let lhs = node.child(1 - 1).unwrap();
    let rhs = node.child(n - 1).unwrap();

    if lhs.kind() == "{" && rhs.kind() != "}" {
        let child = node.child(0).into_result()?;
        let range: Range = child.range().into();
        let message = "unmatched opening brace '{'";
        let diagnostic = Diagnostic::new_simple(range.into(), message.into());
        diagnostics.push(diagnostic);
    }

    true.ok()
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

    if node.kind() != "==" {
        return false.ok();
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let contents = child.utf8_text(context.source.as_bytes()).unwrap();
        if matches!(contents, "NA" | "NaN" | "NULL") {
            let message = match contents {
                "NA" => "consider using `is.na()` to check NA values",
                "NaN" => "consider using `is.nan()` to check NaN values",
                "NULL" => "consider using `is.null()` to check NULL values",
                _ => continue,
            };
            let range: Range = child.range().into();
            let mut diagnostic = Diagnostic::new_simple(range.into(), message.into());
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
    if !matches!(node.kind(), "ERROR") {
        return false.ok();
    }

    let range: Range = node.range().into();
    let text = node.utf8_text(context.source.as_bytes())?;
    let message = format!("Syntax error: unexpected token '{}'", text);
    let diagnostic = Diagnostic::new_simple(range.into(), message.into());
    diagnostics.push(diagnostic);

    true.ok()
}

fn check_unclosed_arguments(
    node: Node,
    _context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    let arguments = unwrap!(node.child_by_field_name("arguments"), None => {
        return false.ok();
    });

    let n = arguments.child_count();
    if n == 0 {
        return false.ok();
    }

    let lhs = arguments.child(1 - 1).unwrap();
    let rhs = arguments.child(n - 1).unwrap();

    if lhs.kind() == "(" && rhs.kind() == ")" {
        return false.ok();
    } else if lhs.kind() == "[" && rhs.kind() == "]" {
        return false.ok();
    } else if lhs.kind() == "[[" && rhs.kind() == "]]" {
        return false.ok();
    }

    let range: Range = lhs.range().into();
    let message = format!("unmatched opening bracket '{}'", lhs.kind());
    let diagnostic = Diagnostic::new_simple(range.into(), message.into());
    diagnostics.push(diagnostic);

    true.ok()
}

fn check_unexpected_assignment_in_if_conditional(
    node: Node,
    _context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool> {
    let n = node.child_count();
    if n == 0 {
        return false.ok();
    }

    let kind = node.kind();
    if kind != "if" {
        return false.ok();
    }

    let condition = unwrap!(node.child_by_field_name("condition"), None => {
        return false.ok();
    });

    if !matches!(condition.kind(), "=") {
        return false.ok();
    }

    let range: Range = condition.range().into();
    let message = "unexpected '='; use '==' to compare values for equality";
    let diagnostic = Diagnostic::new_simple(range.into(), message.into());
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

    // Skip if this isn't an identifier.
    if node.kind() != "identifier" {
        return false.ok();
    }

    // Skip if this identifier belongs to a '$' node.
    if let Some(parent) = node.parent() {
        if parent.kind() == "$" {
            if let Some(rhs) = parent.child_by_field_name("rhs") {
                if rhs == node {
                    return false.ok();
                }
            }
        }
    }

    // Skip if a symbol with this name is in scope.
    let name = node.utf8_text(context.source.as_bytes())?;
    if context.has_definition(name) {
        return false.ok();
    }

    // No symbol in scope; provide a diagnostic.
    let range: Range = node.range().into();
    let identifier = node.utf8_text(context.source.as_bytes())?;
    let message = format!("no symbol named '{}' in scope", identifier);
    let mut diagnostic = Diagnostic::new_simple(range.into(), message.into());
    diagnostic.severity = Some(DiagnosticSeverity::WARNING);
    diagnostics.push(diagnostic);

    true.ok()
}
