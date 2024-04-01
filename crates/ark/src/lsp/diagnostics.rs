//
// diagnostics.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::time::Duration;

use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;
use harp::call::r_expr_quote;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::external_ptr::ExternalPointer;
use harp::object::RObject;
use harp::protect::RProtect;
use harp::r_symbol;
use harp::utils::r_is_null;
use harp::utils::r_symbol_quote_invalid;
use harp::utils::r_symbol_valid;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libr::R_EmptyEnv;
use libr::R_GlobalEnv;
use libr::R_NilValue;
use libr::R_lsInternal;
use libr::Rf_ScalarInteger;
use libr::Rf_allocVector;
use libr::Rf_cons;
use libr::Rf_lang1;
use libr::Rf_xlength;
use libr::CDR;
use libr::ENCLOS;
use libr::RAW;
use libr::RAWSXP;
use libr::SETCDR;
use libr::SET_TAG;
use libr::SET_VECTOR_ELT;
use libr::VECSXP;
use libr::VECTOR_ELT;
use ropey::Rope;
use stdext::*;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::DiagnosticSeverity;
use tower_lsp::lsp_types::Url;
use tree_sitter::Node;
use tree_sitter::Range;

use crate::interface::RMain;
use crate::lsp::backend::Backend;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_tree_sitter_range_to_lsp_range;
use crate::lsp::indexer;
use crate::lsp::traits::rope::RopeExt;
use crate::r_task;
use crate::r_task::r_async_task;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnmatchedDelimiterType;

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

/// Clear the diagnostics of a single file
///
/// Note that we don't reference the `document` in the DashMap in any way,
/// in case it has already been removed by the time the thread runs.
///
/// Must be called from an LSP method so it runs on the LSP tokio `Runtime`
pub fn clear_diagnostics(backend: Backend, uri: Url, version: Option<i32>) {
    tokio::spawn(async move {
        // Empty set to clear them
        let diagnostics = Vec::new();

        backend
            .client
            .publish_diagnostics(uri.clone(), diagnostics, version)
            .await
    });
}

/// Refresh the diagnostics of a single file
///
/// Must be called from an LSP method so it runs on the LSP tokio `Runtime`
pub fn refresh_diagnostics(backend: Backend, uri: Url, version: Option<i32>) {
    tokio::spawn(async move {
        refresh_diagnostics_impl(backend, uri, version).await;
    });
}

/// Request a full diagnostic refresh on all open documents
///
/// Called after each R console execution so diagnostics are dynamic to code sent to the
/// console.
///
/// Still goes through `request_diagnostics()` with its 1 second delay before actually
/// generating diagnostics. This avoids being too aggressive with the refresh, since
/// generating diagnostics does require R.
pub fn refresh_all_open_file_diagnostics() {
    r_async_task(|| {
        let main = RMain::get();

        let Some(backend) = main.get_lsp_backend() else {
            log::error!("No LSP `backend` to request a diagnostic refresh with.");
            return;
        };

        let runtime = main.get_lsp_runtime();

        for document in backend.documents.iter() {
            let backend = backend.clone();
            let uri = document.key().clone();
            let version = document.version.clone();

            // Explicit `drop()` before we request diagnostics, which requires `get()`ting
            // this document again on the thread. Likely not needed, but better to be safe.
            drop(document);

            runtime.spawn(async move {
                refresh_diagnostics_impl(backend, uri, version).await;
            });
        }
    });
}

async fn refresh_diagnostics_impl(backend: Backend, uri: Url, version: Option<i32>) {
    let diagnostics = match request_diagnostics(&backend, &uri).await {
        Ok(diagnostics) => diagnostics,
        Err(err) => {
            log::error!("While refreshing diagnostics for '{uri}': {err:?}");
            return;
        },
    };

    let Some(diagnostics) = diagnostics else {
        // File was closed or `version` changed. Not an error, just a side effect
        // of delaying diagnostics.
        return;
    };

    backend
        .client
        .publish_diagnostics(uri.clone(), diagnostics, version)
        .await
}

async fn request_diagnostics(
    backend: &Backend,
    uri: &Url,
) -> anyhow::Result<Option<Vec<Diagnostic>>> {
    // SAFETY: It is absolutely imperative that the `doc` be `Drop`ped outside
    // of any `await` context. That is why the extraction of `doc` is captured
    // inside of `try_generate_diagnostics()` and `get_diagnostics_id()`; this ensures
    // that any `doc` is `Drop`ped before the `sleep().await` call. If this doesn't
    // happen, then the `await` could switch us to a different LSP task, which will also
    // try and access a document, causing a deadlock since it won't be able to access a
    // document until our `doc` reference is dropped, but we can't drop until we get
    // control back from the `await`.

    // Get the `diagnostics_id` for this request, before sleeping
    let diagnostics_id = get_diagnostics_id(backend, uri)?;

    // Wait some amount of time. Note that the `diagnostics_id` is updated on every
    // diagnostic request, so if another request comes in while this task is waiting,
    // we'll see that the current `diagnostics_id` is now past the id associated with this
    // request and toss it away.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    Ok(try_generate_diagnostics(backend, uri, diagnostics_id))
}

fn get_diagnostics_id(backend: &Backend, uri: &Url) -> anyhow::Result<i64> {
    let Some(mut document) = backend.documents.get_mut(uri) else {
        return Err(anyhow!("Unknown document URI '{uri}'."));
    };

    // First, bump the id to correspond to this request
    document.diagnostics_id += 1;

    // Return the bumped id
    Ok(document.diagnostics_id)
}

fn try_generate_diagnostics(
    backend: &Backend,
    uri: &Url,
    diagnostics_id: i64,
) -> Option<Vec<Diagnostic>> {
    // Get reference to document.
    // At this point we already know this document existed before we slept, so if it
    // doesn't exist now, that is because it must have been closed, so if that occurs
    // then simply return.
    let Some(doc) = backend.documents.get(uri) else {
        log::info!("Document with uri '{uri}' no longer exists after diagnostics delay. It was likely closed.");
        return None;
    };

    // Check if the `diagnostics_id` has been bumped by another diagnostics request while
    // we were asleep
    let current_diagnostics_id = doc.diagnostics_id;
    if diagnostics_id != current_diagnostics_id {
        // log::info!("[diagnostics({diagnostics_id}, {uri})] Aborting diagnostics in favor of id {current_diagnostics_id}.");
        return None;
    }

    // If we've made it this far, we really do want diagnostics, and we want them to
    // be accurate. The indexer is a very important part of our diagnostics, so we need
    // it to finish an initial run before we generate any diagnostics, otherwise they
    // can be pretty bad and annoying. Importantly, we place this check after the 1 sec
    // timeout delay and version check to ensure that the `lock()` doesn't run needlessly.
    backend.indexer_state_manager.wait_until_initialized();

    Some(generate_diagnostics(&doc))
}

fn generate_diagnostics(doc: &Document) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

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

        r_task(|| unsafe {
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
        });

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
            BinaryOperatorType::LeftSuperAssignment => {
                recurse_superassignment(node, context, diagnostics)
            },
            BinaryOperatorType::LeftAssignment => recurse_assignment(node, context, diagnostics),
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
        if lhs.is_identifier_or_string() {
            let name = context.contents.node_slice(&lhs)?.to_string();
            let range = lhs.range();
            context.add_defined_variable(name.as_str(), range);
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

        context.add_defined_variable(symbol.as_str(), location.into());
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

struct TreeSitterCall<'a> {
    // A call of the form <fun>(list(0L, <ptr>), foo = list(1L, <ptr>))
    pub call: RObject,
    node_phantom: PhantomData<&'a Node<'a>>,
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
                    // TODO: Wrap this in a nice constructor
                    let node_size = std::mem::size_of::<Node>();
                    let node_storage = Rf_allocVector(RAWSXP, node_size as isize);
                    SET_VECTOR_ELT(*arg_list, 1, node_storage);

                    let p_node_storage: *mut Node<'a> = RAW(node_storage) as *mut Node<'a>;
                    std::ptr::copy_nonoverlapping(&value, p_node_storage, 1);
                }

                SETCDR(tail, Rf_cons(*arg_list, R_NilValue));
                tail = CDR(tail);

                // potentially add the argument name
                if let Some(name) = child.child_by_field_name("name") {
                    let name = context.contents.node_slice(&name)?.to_string();
                    let sym_name = r_symbol!(name);
                    SET_TAG(tail, sym_name);
                }

                i = i + 1;
            }
        }

        Ok(Self {
            call,
            node_phantom: PhantomData,
        })
    }
}

fn recurse_call_arguments_custom(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    diagnostic_function: &str,
) -> Result<()> {
    r_task(|| unsafe {
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
            .add(r_expr_quote(call.call))
            .add(ExternalPointer::new(context.contents))
            .call()?;

        if !r_is_null(*custom_diagnostics) {
            let n = Rf_xlength(*custom_diagnostics);
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
                let value: Node<'static> = *(RAW(ptr) as *mut Node<'static>);

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
                    let range = value.range();
                    let range = convert_tree_sitter_range_to_lsp_range(context.contents, range);
                    let diagnostic = Diagnostic::new_simple(range, message.into());
                    diagnostics.push(diagnostic);
                }
            }
        }

        ().ok()
    })
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
    let diagnostic = Diagnostic::new_simple(range, message.into());
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
    let diagnostic = Diagnostic::new_simple(range, message.into());
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
    let message = format!("unmatched opening bracket '{}'", lhs.kind());
    let diagnostic = Diagnostic::new_simple(range, message.into());
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
    let mut diagnostic = Diagnostic::new_simple(range, message.into());
    diagnostic.severity = Some(DiagnosticSeverity::WARNING);
    diagnostics.push(diagnostic);

    true.ok()
}

#[cfg(test)]
mod tests {

    use tower_lsp::lsp_types::Position;

    use crate::lsp::diagnostics::generate_diagnostics;
    use crate::lsp::diagnostics::is_unmatched_block;
    use crate::lsp::documents::Document;
    use crate::test::r_test;

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
            let diagnostics = generate_diagnostics(&document);
            assert!(diagnostics.is_empty());
        })
    }

    #[test]
    fn test_expression_after_call_argument() {
        r_test(|| {
            let text = "match(1, 2 3)";
            let document = Document::new(text, None);

            let diagnostics = generate_diagnostics(&document);
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
}
