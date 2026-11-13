//
// diagnostics.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use anyhow::bail;
use anyhow::Result;
use camino::Utf8Path;
use oak_core::identifier::sym_quote_invalid;
use oak_db::Environment;
use oak_db::EnvironmentStatus;
use oak_db::File;
use oak_db::RootKind;
use oak_db::Severity as OakSeverity;
use stdext::*;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticRelatedInformation;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use tower_lsp_server::ls_types::Location;
use tower_lsp_server::ls_types::NumberOrString;
use tower_lsp_server::ls_types::Uri;
use tree_sitter::Node;
use tree_sitter::Point;
use tree_sitter::Range;

use crate::lsp::coordinates::lsp_range_from_tree_sitter_range;
use crate::lsp::db::FileLegacyExt;
use crate::lsp::db::LegacyDb;
use crate::lsp::declarations::top_level_declare;
use crate::lsp::diagnostics_syntax::missing_namespace_rhs_diagnostics;
use crate::lsp::diagnostics_syntax::syntax_diagnostics;
use crate::lsp::indexer;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::node_has_error_or_missing;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsConfig {
    pub enable: bool,

    /// Whether to publish diagnostics whose `DiagnosticKind::is_experimental()`
    /// is `true`.
    pub experimental: bool,
}

#[derive(Clone)]
pub struct DiagnosticContext<'a> {
    pub(crate) db: &'a dyn LegacyDb,
    pub(crate) file: File,
    pub(crate) encoding: PositionEncoding,
    pub(crate) uri: &'a Uri,

    /// The symbols currently defined and available in the session.
    pub session_symbols: HashSet<String>,

    /// The symbols used within the document, as a 'stack' of symbols,
    /// mapping symbol names to the locations where they were defined.
    pub document_symbols: Vec<HashMap<String, Range>>,

    /// The symbols defined in the workspace.
    pub workspace_symbols: HashSet<String>,

    /// The symbols exported by packages loaded via `library()` calls in this
    /// document. Currently global.
    pub library_symbols: BTreeMap<Point, HashSet<String>>,

    // Whether or not we're inside of a formula.
    pub in_formula: bool,

    // Whether or not we're inside of a call-like node's arguments
    pub in_call_like_arguments: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enable: true,
            experimental: false,
        }
    }
}

impl<'a> DiagnosticContext<'a> {
    pub(crate) fn contents(&self) -> &'a str {
        self.file.source_text(self.db).as_str()
    }

    pub(crate) fn new(
        db: &'a dyn LegacyDb,
        file: File,
        encoding: PositionEncoding,
        uri: &'a Uri,
    ) -> Self {
        Self {
            file,
            encoding,
            db,
            uri,
            document_symbols: Vec::new(),
            session_symbols: HashSet::new(),
            workspace_symbols: HashSet::new(),
            library_symbols: BTreeMap::new(),
            in_formula: false,
            in_call_like_arguments: false,
        }
    }

    pub fn add_defined_variable(&mut self, name: &str, location: Range) {
        let symbols = self.document_symbols.last_mut().unwrap();
        symbols.insert(name.to_string(), location);
    }

    pub fn has_definition(&self, name: &str, start_position: Point) -> bool {
        // Check document symbols
        for symbols in &self.document_symbols {
            if symbols.contains_key(name) {
                return true;
            }
        }

        // Check workspace symbols
        if self.workspace_symbols.contains(name) {
            return true;
        }

        // Check all symbols exported by `library()` calls before the given position
        for (library_position, exports) in self.library_symbols.iter() {
            if *library_position > start_position {
                break;
            }
            if exports.contains(name) {
                return true;
            }
        }

        // Finally, check session symbols
        self.session_symbols.contains(name)
    }
}

/// Diagnose `file`. Related information locations point to `uri`.
///
/// `session_scopes` lists names in the global environment and each environment
/// on the search path, in that order. All names count as defined symbols,
/// regardless of their environment.
pub(crate) fn generate_diagnostics(
    db: &dyn LegacyDb,
    file: File,
    uri: &Uri,
    config: &DiagnosticsConfig,
    encoding: PositionEncoding,
    session_scopes: &[Vec<String>],
    testthat: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if !config.enable {
        return diagnostics;
    }

    // Library files are foreign code the user can't fix (installed packages,
    // fetched CRAN/base sources), so diagnostics don't apply (even though
    // symbol navigation works).
    if file
        .root(db)
        .is_some_and(|root| root.kind(db) == RootKind::Library)
    {
        return diagnostics;
    }

    // Check that diagnostics are not disabled in top-level declarations for
    // this document
    let tree = file.tree_sitter(db);
    let contents = file.source_text(db).as_str();
    let decls = top_level_declare(tree, contents);
    if !decls.diagnostics {
        return diagnostics;
    }

    let mut context = DiagnosticContext::new(db, file, encoding, uri);

    // All cells share the notebook's environment, so only the first cell reports
    // an environment failure. Non-cell files each report their environment failure.
    let environment = file.environment(db);
    if file.earlier_notebook_cells(db).is_empty() {
        diagnostics.extend(environment_failure_diagnostic(db, environment));
    }

    // Collect syntax related diagnostics for `ERROR` and `MISSING` nodes
    let root = file.tree_sitter(db).root_node();
    match syntax_diagnostics(root, &context) {
        Ok(mut syntax_diagnostics) => diagnostics.append(&mut syntax_diagnostics),
        Err(err) => log::error!("Error while generating syntax diagnostics: {err:?}"),
    }

    // Without resolved libraries, semantic checks can falsely report missing
    // packages and exports in `pkg::` expressions and `library()` calls.
    // After a successful resolution, a failed rerun preserves the libraries,
    // so semantic checks continue. Only those checks need the symbol context
    // built below.
    //
    // `pkg::` without a right-hand side parses without an error node, so the
    // syntax pass misses it and the semantic walk reports it instead. Without
    // that walk, report it here.
    if *environment.status(db) == EnvironmentStatus::Unresolved {
        match missing_namespace_rhs_diagnostics(root, &context) {
            Ok(mut namespace_diagnostics) => diagnostics.append(&mut namespace_diagnostics),
            Err(err) => log::error!("Error while generating namespace diagnostics: {err:?}"),
        }
        return diagnostics;
    }

    // Add a 'root' context for the document.
    context.document_symbols.push(HashMap::new());

    context
        .workspace_symbols
        .extend(indexer::workspace_symbol_names(db).iter().cloned());

    // If this is a package, add imported symbols to workspace
    if let Some(package) = file.package(db) {
        let namespace = package.namespace(db);

        // Add symbols from `importFrom()` directives
        for import in &namespace.imports {
            context.workspace_symbols.insert(import.name.clone());
        }

        // Add symbols from `import()` directives
        for package_import in &namespace.package_imports {
            if let Some(pkg) = db.package_by_name(package_import) {
                for export in &pkg.namespace(db).exports {
                    context.workspace_symbols.insert(export.clone());
                }
            }
        }
    }

    // Simple workaround to include testthat exports in test files. I think the
    // general principle would be that (a) files in `tests/testthat/` include
    // `testthat.R` as a preamble (note that people modify that file e.g. to add
    // more `library()` calls), and (b) all helper files are included in a
    // test-specific workspace (which is effectively the case currently as we
    // don't special-case how workspace inclusion works for packages). We might
    // want to provide a mechanism for test packages to declare this sort of
    // test files setup.
    if testthat {
        if let Some(pkg) = db.package_by_name("testthat") {
            for export in &pkg.namespace(db).exports {
                context.workspace_symbols.insert(export.clone());
            }
        }
    }

    // Add per-environment session symbols
    for scope in session_scopes {
        for name in scope.iter() {
            context
                .session_symbols
                .insert(sym_quote_invalid(name.as_str()));
        }
    }

    // Add `_` pipe placeholder as a "known" session symbol so we never flag it with
    // "symbol not found" when it shows up as an `identifier` node
    context.session_symbols.insert(String::from("_"));

    // In a notebook cell, packages that earlier cells attach are available
    // from the start of this cell. Later cells don't count.
    for cell in file.earlier_notebook_cells(db) {
        for package in cell.attached_packages(db) {
            attach_package(package.text(db).as_str(), Point::new(0, 0), &mut context);
        }
    }

    // Collect semantic related diagnostics
    match semantic_diagnostics(root, &mut context) {
        Ok(mut semantic_diagnostics) => diagnostics.append(&mut semantic_diagnostics),
        Err(err) => log::error!("Error while generating semantic diagnostics: {err:?}"),
    }

    // Collect diagnostics from `oak_db`'s semantic index
    match oak_diagnostics(&context, config.experimental) {
        Ok(mut oak_diagnostics) => diagnostics.append(&mut oak_diagnostics),
        Err(err) => log::error!("Error while generating oak diagnostics: {err:?}"),
    }

    diagnostics
}

/// Report the environment's last recorded startup failure at the start of
/// the file. An unresolved environment gets an information diagnostic.
/// A resolved environment gets a warning. Return `None` if no error is recorded.
///
/// Blank lines separate the summary, error, and retry instructions so a
/// multiline error, such as an exit status followed by R's `stderr`, stays
/// distinct from the instructions.
///
/// For workspace environments, check `.Rprofile` and `.Renviron` on disk when
/// generating the diagnostic, so the hint reflects files created or deleted
/// since the failed run.
fn environment_failure_diagnostic(
    db: &dyn LegacyDb,
    environment: Environment,
) -> Option<Diagnostic> {
    let error = environment.last_error(db).as_deref()?;

    let (severity, consequence) = match environment.status(db) {
        EnvironmentStatus::Unresolved => {
            (DiagnosticSeverity::INFORMATION, "only syntax is checked")
        },
        EnvironmentStatus::Resolved(_) => (
            DiagnosticSeverity::WARNING,
            "libraries come from the last successful startup",
        ),
    };

    let (location, mut startup_files, mut fixes) = match environment.dir(db) {
        // `FilePath` displays as a URL, so show the bare path when there is one.
        Some(dir) => match dir.as_path() {
            Some(path) => (
                format!("in {path}"),
                existing_startup_files(path),
                WORKSPACE_ENVIRONMENT_FIXES,
            ),
            None => (format!("in {dir}"), Vec::new(), WORKSPACE_ENVIRONMENT_FIXES),
        },
        // Changes to `.Rprofile` and `.Renviron` only rerun workspace
        // environments. Retrying the default requires restarting R, which
        // restarts the language server.
        None => (
            String::from("for the default environment (files outside workspace folders)"),
            Vec::new(),
            DEFAULT_ENVIRONMENT_FIXES,
        ),
    };

    // Without an R executable, no startup file ran, and installing R takes
    // effect only after a restart.
    if error == NO_R_EXECUTABLE {
        startup_files.clear();
        fixes = DEFAULT_ENVIRONMENT_FIXES;
    }

    let mut message = format!("R startup failed {location}, so {consequence}.");
    if !startup_files.is_empty() {
        message.push_str(&format!(" See {}.", startup_files.join(" and ")));
    }
    message.push_str("\n\n");
    message.push_str(error);
    message.push_str("\n\n");
    message.push_str(fixes);

    // The default range is empty at the start of the file.
    Some(Diagnostic {
        severity: Some(severity),
        source: Some("ark".to_string()),
        message,
        ..Default::default()
    })
}

fn existing_startup_files(dir: &Utf8Path) -> Vec<String> {
    [".Rprofile", ".Renviron"]
        .into_iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .map(|path| path.to_string())
        .collect()
}

/// Failure message for environment resolution when no R executable is found.
/// [`environment_failure_diagnostic()`] matches this exact text to omit startup
/// file hints and recommend restarting R after installing it.
pub(crate) const NO_R_EXECUTABLE: &str = "no R executable was found";

pub(crate) const WORKSPACE_ENVIRONMENT_FIXES: &str = "Edits to .Rprofile or .Renviron in this directory are picked up automatically. Other fixes, such as installing R or renv, or editing ~/.Rprofile, take effect after you restart R.";

pub(crate) const DEFAULT_ENVIRONMENT_FIXES: &str =
    "Fixes, such as installing R or editing ~/.Rprofile, take effect after you restart R.";

/// Convert `oak_db`'s file-level diagnostics into LSP diagnostics.
fn oak_diagnostics(
    context: &DiagnosticContext,
    experimental: bool,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    for diagnostic in context.file.diagnostics(context.db) {
        if diagnostic.kind().is_experimental() && !experimental {
            continue;
        }

        let range = to_proto::range(
            diagnostic.range(),
            context.file.line_index(context.db),
            context.encoding,
        )?;

        let related_information = if diagnostic.annotations().is_empty() {
            None
        } else {
            Some(
                diagnostic
                    .annotations()
                    .iter()
                    .map(|annotation| {
                        let range = to_proto::range(
                            annotation.range,
                            context.file.line_index(context.db),
                            context.encoding,
                        )?;
                        Ok(DiagnosticRelatedInformation {
                            location: Location {
                                uri: context.uri.clone(),
                                range,
                            },
                            message: annotation.message.clone(),
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            )
        };

        diagnostics.push(Diagnostic {
            range,
            severity: Some(oak_severity_to_lsp(diagnostic.kind().severity())),
            code: Some(NumberOrString::String(
                diagnostic.kind().as_str().to_string(),
            )),
            source: Some("ark".to_string()),
            message: diagnostic.message().to_string(),
            related_information,
            ..Default::default()
        });
    }

    Ok(diagnostics)
}

fn oak_severity_to_lsp(severity: OakSeverity) -> DiagnosticSeverity {
    match severity {
        OakSeverity::Error => DiagnosticSeverity::ERROR,
        OakSeverity::Warning => DiagnosticSeverity::WARNING,
        OakSeverity::Info => DiagnosticSeverity::INFORMATION,
        OakSeverity::Hint => DiagnosticSeverity::HINT,
    }
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
        NodeType::Subset | NodeType::Subset2 => {
            recurse_subset_or_subset2(node, context, diagnostics)
        },
        NodeType::Call => recurse_call(node, context, diagnostics),
        NodeType::UnaryOperator(UnaryOperatorType::Tilde) => {
            recurse_formula(node, context, diagnostics)
        },
        NodeType::UnaryOperator(_) => recurse_default(node, context, diagnostics),
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
        let name = variable.node_as_str(context.contents())?;
        let range = variable.range();
        context.add_defined_variable(name, range);
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

    let name = identifier.node_as_str(context.contents())?;
    let range = identifier.range();
    context.add_defined_variable(name, range);

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

    let dot = dot.node_as_str(context.contents())?;
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
            let name = name.node_as_str(context.contents())?;
            context.add_defined_variable(name, range);
            continue;
        };

        let Some(value) = child.child_by_field_name("value") else {
            continue;
        };

        // i.e. `.[x, y]` where `value` is just a name that dotty assigns to
        if value.is_identifier() {
            let range = value.range();
            let name = value.node_as_str(context.contents())?;
            context.add_defined_variable(name, range);
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
    if node.is_magrittr_pipe_operator(context.contents())? {
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

    // Treat workspace packages as available even when they aren't installed.
    // This applies to `pkg::` references anywhere, not just inside the package.
    let package = lhs.node_as_str(context.contents())?;
    if context.db.package_by_name(package).is_none() {
        let range = lhs.range();
        let range = lsp_range_from_tree_sitter_range(
            range,
            context.file.line_index(context.db),
            context.encoding,
        )?;
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

        let symbol = name.node_as_str(context.contents())?;
        let location = name.range();

        context.add_defined_variable(symbol, location);
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
    let Some(body) = node.child_by_field_name("body") else {
        // Would be unexpected, grammar requires exactly 1 `body`
        return Ok(());
    };
    recurse(body, context, diagnostics)
}

/// Default recursion for arguments of a call-like node
///
/// This applies for function calls, subset, and subset2, all of which are guaranteed
/// to have the same tree-sitter node structure (i.e., with an `arguments` child).
fn recurse_call_like_arguments_default(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<()> {
    // TODO: Can we better handle NSE in things like `quote()` and
    // `dplyr::mutate()` so we don't have to turn off certain diagnostics when
    // we are inside a call's arguments?

    // We used to clone `context` here to prevent assignments from applying to
    // the caller. We now purposely preserve the caller's context with its
    // `document_symbols`. We explicitly want to index symbols assigned in the
    // call arguments to avoid false positive lints about missing symbols, see
    // https://github.com/posit-dev/positron/issues/3048.
    //
    // Because of the way we traverse the syntax tree, this makes the assumption
    // that execution order is deterministic from left to right. This is not
    // necessarily correct since arguments are lazily evaluated, and whether
    // this is true depends on the function's implementation. For now we assume
    // every function behaves like `list()`, which is our default model of
    // strict evaluation.
    with_in_call_like_arguments(context, |context| {
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return Ok(());
        };

        // Iterate over and recurse into `arguments`
        let mut cursor = arguments.walk();
        let children = arguments.children_by_field_name("argument", &mut cursor);

        for child in children {
            // Recurse into `value`s
            if let Some(value) = child.child_by_field_name("value") {
                recurse(value, context, diagnostics)?;
            }
        }

        Ok(())
    })
}

fn with_in_call_like_arguments<F>(context: &mut DiagnosticContext, f: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut DiagnosticContext) -> anyhow::Result<()>,
{
    // Save `in_call_like_arguments` to restore it on exit. Necessary to handle nested calls
    // and maintain the state to `true` until we've left the outermost call.
    let in_call_like_arguments = context.in_call_like_arguments;
    context.in_call_like_arguments = true;

    let result = f(context);

    // Reset on exit
    context.in_call_like_arguments = in_call_like_arguments;
    result
}

fn recurse_call(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Run diagnostics on the call itself
    dispatch(node, context, diagnostics);

    // Recurse into the callee
    let Some(callee) = node.child_by_field_name("function") else {
        return Ok(());
    };
    recurse(callee, context, diagnostics)?;

    // dispatch based on the function
    //
    // TODO: Handle certain 'scope-generating' function calls, e.g.
    // things like 'local({ ... })'.
    let fun = callee.node_as_str(context.contents())?;

    match fun {
        "library" | "require" => {
            // Track symbols exported by `library()` or `require()` calls
            if let Err(err) = handle_package_attach_call(node, context) {
                log::warn!("Can't handle attach call: {err:?}");
            }
        },
        _ => {},
    };

    // Continue with default recursion to handle any other arguments
    recurse_call_like_arguments_default(node, context, diagnostics)?;

    ().ok()
}

fn handle_package_attach_call(node: Node, context: &mut DiagnosticContext) -> anyhow::Result<()> {
    // Find the first argument (package name). Positionally for now, no attempt
    // at argument matching whatsoever.
    let Some(package_node) = node.arguments_values().flatten().next() else {
        return Err(anyhow::anyhow!("Can't unpack attached package argument"));
    };

    // Just bail if `character.only` is passed, even if it's actually `FALSE`.
    // We'll do better when we have a more capable argument inspection
    // infrastructure.
    if node
        .arguments_names_as_string(context.contents())
        .flatten()
        .any(|n| n == "character.only")
    {
        return Ok(());
    }

    let package_name = package_node.get_identifier_or_string_text(context.contents())?;
    attach_package(package_name, node.end_position(), context);

    Ok(())
}

/// Record the exports that `library(package_name)` makes available from
/// `attach_pos` onwards, including packages it attaches in turn.
fn attach_package(package_name: &str, attach_pos: Point, context: &mut DiagnosticContext) {
    insert_package_exports(package_name, attach_pos, context);

    // Also attach packages from `Depends` field, if any
    if let Some(package_names) = context
        .db
        .package_by_name(package_name)
        .and_then(|package| package.depends(context.db).as_ref())
    {
        for package_name in package_names.iter() {
            insert_package_exports(package_name, attach_pos, context);
        }
    }

    // Special handling for the tidyverse and tidymodels packages. Hard-coded
    // for now but in the future, this should probably be expressed as a
    // `DESCRIPTION` field like `Config/Needs/attach`.
    let attach_field = match package_name {
        // https://github.com/tidyverse/tidyverse/blob/0231aafb/R/attach.R#L1
        "tidyverse" => {
            vec![
                "dplyr",
                "readr",
                "forcats",
                "stringr",
                "ggplot2",
                "tibble",
                "lubridate",
                "tidyr",
                "purrr",
            ]
        },
        // https://github.com/tidymodels/tidymodels/blob/aa3f82cf/R/attach.R#L1
        "tidymodels" => {
            vec![
                "broom",
                "dials",
                "dplyr",
                "ggplot2",
                "infer",
                "modeldata",
                "parsnip",
                "purrr",
                "recipes",
                "rsample",
                "tibble",
                "tidyr",
                "tune",
                "workflows",
                "workflowsets",
                "yardstick",
            ]
        },
        _ => vec![],
    };
    for package_name in attach_field {
        insert_package_exports(package_name, attach_pos, context);
    }
}

fn insert_package_exports(package_name: &str, attach_pos: Point, context: &mut DiagnosticContext) {
    let Some(package) = context.db.package_by_name(package_name) else {
        // Package is not installed. This is linted via another path.
        return;
    };

    // Start from explicit `NAMESPACE` exports
    let mut exports = package.namespace(context.db).exports.clone().into_vec();

    // Add all documented symbols. This should cover documented datasets to avoid some
    // false positives in favor of allowing some false negatives. This is admittedly a
    // bit of a stopgap!
    if let Some(index) = package.index(context.db) {
        exports.extend(index.names().iter().cloned());
    }

    // No need to worry about sorting or deduplicating, the `HashSet` takes care of this
    context
        .library_symbols
        .entry(attach_pos)
        .or_default()
        .extend(exports);
}

fn recurse_subset_or_subset2(
    node: Node,
    context: &mut DiagnosticContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    // Run diagnostics on the call itself
    dispatch(node, context, diagnostics);

    // Recurse into the callee
    let Some(callee) = node.child_by_field_name("function") else {
        return Ok(());
    };
    recurse(callee, context, diagnostics)?;

    recurse_call_like_arguments_default(node, context, diagnostics)
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
        let contents = child.node_as_str(context.contents())?;

        if matches!(contents, "NA" | "NaN" | "NULL") {
            let message = match contents {
                "NA" => "Consider using `is.na()` to check `NA` values.",
                "NaN" => "Consider using `is.nan()` to check `NaN` values.",
                "NULL" => "Consider using `is.null()` to check `NULL` values.",
                _ => continue,
            };
            let range = child.range();
            let range = lsp_range_from_tree_sitter_range(
                range,
                context.file.line_index(context.db),
                context.encoding,
            )?;
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
    let range = lsp_range_from_tree_sitter_range(
        range,
        context.file.line_index(context.db),
        context.encoding,
    )?;
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

    // Skip if we're working on the arguments of a call-like node
    if context.in_call_like_arguments {
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
    let name = node.node_as_str(context.contents())?;
    if context.has_definition(name, node.start_position()) {
        return false.ok();
    }

    // No symbol in scope; provide a diagnostic.
    let range = node.range();
    let range = lsp_range_from_tree_sitter_range(
        range,
        context.file.line_index(context.db),
        context.encoding,
    )?;
    let identifier = node.node_as_str(context.contents())?;
    let message = format!("No symbol named '{}' in scope.", identifier);
    let mut diagnostic = Diagnostic::new_simple(range, message);
    diagnostic.severity = Some(DiagnosticSeverity::WARNING);
    diagnostics.push(diagnostic);

    true.ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use aether_lsp_utils::proto::PositionEncoding;
    use aether_path::FilePath;
    use biome_line_index::WideEncoding;
    use oak_db::DbInputs;
    use oak_db::File;
    use oak_db::OakDatabase;
    use oak_db::SourceDb;
    use oak_scan::DbScan;
    use oak_scan::ScanScheduler;
    use tempfile::TempDir;
    use tower_lsp_server::ls_types as lsp_types;
    use tower_lsp_server::ls_types::Position;
    use tower_lsp_server::ls_types::Uri;

    use super::DiagnosticsConfig;
    use super::DEFAULT_ENVIRONMENT_FIXES;
    use super::NO_R_EXECUTABLE;

    #[derive(Default)]
    struct TestState {
        db: OakDatabase,
        session_scopes: Vec<Vec<String>>,
        config: DiagnosticsConfig,
    }

    impl TestState {
        fn with_db(db: OakDatabase) -> Self {
            Self {
                db,
                ..Default::default()
            }
        }

        fn diagnose(&self, file: File, uri: &Uri) -> Vec<lsp_types::Diagnostic> {
            super::generate_diagnostics(
                &self.db,
                file,
                uri,
                &self.config,
                PositionEncoding::Wide(WideEncoding::Utf16),
                &self.session_scopes,
                false,
            )
        }
    }

    fn generate_diagnostics(code: &str, state: &TestState) -> Vec<lsp_types::Diagnostic> {
        let url = url::Url::parse("file:///test.R").unwrap();
        let uri: Uri = url.as_str().parse().unwrap();
        let file = oak_db::File::new(
            &state.db,
            FilePath::from_url(&url),
            oak_db::FileRevision::zero(),
            Some(code.to_string()),
            None,
        );
        state.diagnose(file, &uri)
    }

    /// Scan `paths` as the workspace folders until no scan is pending.
    fn set_workspace_paths(db: &mut OakDatabase, paths: &[PathBuf]) {
        let mut scheduler = ScanScheduler::new();
        let mut requests = scheduler.set_workspace_paths(db, paths);
        while let Some(request) = requests.pop() {
            let result = request.run();
            requests.extend(scheduler.apply_scan_completed(db, result));
        }
    }

    /// Install a package named `name` exporting `exports` into the library
    /// directory `library`, writing the `DESCRIPTION` and `NAMESPACE` files
    /// that the library scanner reads.
    fn install_package(library: &Path, name: &str, exports: &[&str]) {
        let package = library.join(name);
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("DESCRIPTION"), format!("Package: {name}\n")).unwrap();
        let namespace: String = exports
            .iter()
            .map(|export| format!("export({export})\n"))
            .collect();
        std::fs::write(package.join("NAMESPACE"), namespace).unwrap();
        oak_package_metadata::tests::mark_installed(&package);
    }

    #[test]
    fn test_library_static_exports() {
        // `mockpkg` exports `foo` and `bar`
        let library = TempDir::new().unwrap();
        install_package(library.path(), "mockpkg", &["foo", "bar"]);

        let mut db = OakDatabase::new();
        db.set_library_paths(&[library.path().to_path_buf()]);

        // Whereas `current_state()` returns a state with the base package
        // attached, this world state only contains `mockpkg` as an installed
        // package and `library()` on the search path.
        let mut state = TestState::with_db(db);
        state.session_scopes = vec![vec!["library".to_string()]];

        // Test that exported symbols are recognized
        let code = "
                library(mockpkg)
                foo()
                bar
            ";
        let diagnostics = generate_diagnostics(code, &state);

        assert_eq!(diagnostics.len(), 0);

        // Test that non-exported symbols still generate diagnostics
        let code = "
                library('mockpkg')
                undefined()
                also_undefined
            ";

        let diagnostics = generate_diagnostics(code, &state);
        assert_eq!(diagnostics.len(), 2);

        assert!(diagnostics
            .first()
            .unwrap()
            .message
            .contains("No symbol named 'undefined' in scope"));
        assert!(diagnostics
            .get(1)
            .unwrap()
            .message
            .contains("No symbol named 'also_undefined' in scope"));

        // Test duplicate call
        let code = "
                library(mockpkg)
                library(mockpkg)  # duplicate is fine
                foo()
                bar
            ";
        let diagnostics = generate_diagnostics(code, &state);
        assert_eq!(diagnostics.len(), 0);

        // If the library call includes the `character.only` argument, we bail
        let code = r#"
                library(mockpkg, character.only = TRUE)
                foo()
            "#;
        let diagnostics = generate_diagnostics(code, &state);
        assert_eq!(diagnostics.len(), 1);

        // Same if passed `FALSE`, we're not trying to be smart (yet)
        let code = r#"
                library(mockpkg, character.only = FALSE)
                foo()
            "#;
        let diagnostics = generate_diagnostics(code, &state);
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn test_library_static_exports_multiple_packages() {
        // pkg1 exports `foo` and `bar`, pkg2 exports `bar` and `baz`
        let library = TempDir::new().unwrap();
        install_package(library.path(), "pkg1", &["foo", "bar"]);
        install_package(library.path(), "pkg2", &["bar", "baz"]);

        let mut db = OakDatabase::new();
        db.set_library_paths(&[library.path().to_path_buf()]);

        let mut state = TestState::with_db(db);
        state.session_scopes = vec![vec!["library".to_string()]];

        // Code with two library calls at different points
        let code = "
                    foo           # not in scope
                    bar           # not in scope
                    baz           # not in scope

                    library(pkg1)
                    foo           # in scope
                    bar           # in scope
                    baz           # not in scope

                    library(pkg2)
                    foo           # in scope
                    bar           # in scope
                    baz           # in scope
                ";
        let diagnostics = generate_diagnostics(code, &state);

        let messages: Vec<_> = diagnostics.iter().map(|d| d.message.clone()).collect();
        assert!(messages.iter().any(|m| m.contains("No symbol named 'foo'")));
        assert!(messages.iter().any(|m| m.contains("No symbol named 'bar'")));
        assert!(messages.iter().any(|m| m.contains("No symbol named 'baz'")));
        assert!(messages.iter().any(|m| m.contains("No symbol named 'baz'")));
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn test_library_static_exports_require() {
        // `pkg` exports `foo` and `bar`
        let library = TempDir::new().unwrap();
        install_package(library.path(), "pkg", &["foo", "bar"]);

        let mut db = OakDatabase::new();
        db.set_library_paths(&[library.path().to_path_buf()]);

        let mut state = TestState::with_db(db);
        state.session_scopes = vec![vec!["require".to_string()]];

        let code = "
                    foo()
                    require(pkg)
                    bar
                    foo()
                ";
        let diagnostics = generate_diagnostics(code, &state);
        assert!(diagnostics
            .iter()
            .any(|d| d.message.contains("No symbol named 'foo'")));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn test_namespace_library_package_not_flagged() {
        let library = TempDir::new().unwrap();
        install_package(library.path(), "mockpkg", &["foo"]);

        let mut db = OakDatabase::new();
        db.set_library_paths(&[library.path().to_path_buf()]);
        let state = TestState::with_db(db);

        let diagnostics = generate_diagnostics("mockpkg::foo", &state);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_namespace_unknown_package_flagged() {
        let state = TestState::with_db(OakDatabase::new());

        for code in ["mockpkg::foo", "mockpkg:::foo"] {
            let diagnostics = generate_diagnostics(code, &state);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(
                diagnostics[0].message,
                "Package 'mockpkg' is not installed."
            );
        }
    }

    #[test]
    fn test_namespace_workspace_package_not_flagged() {
        let workspace = TempDir::new().unwrap();
        std::fs::write(
            workspace.path().join("DESCRIPTION"),
            "Package: devpkg\nVersion: 1.0.0\n",
        )
        .unwrap();

        let mut state = TestState::with_db(OakDatabase::new());
        set_workspace_paths(&mut state.db, &[workspace.path().to_path_buf()]);

        let diagnostics = generate_diagnostics("devpkg::foo", &state);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_penguins_symbol_no_diagnostic() {
        let library = oak_package_metadata::tests::temp_palmerpenguin();

        let mut db = OakDatabase::new();
        db.set_library_paths(&[library.path().to_path_buf()]);

        // Simulate a world state with the penguins package installed and
        // `library()` on the search path
        let mut state = TestState::with_db(db);
        state.session_scopes = vec![vec!["library".to_string()]];

        let code = r#"
                library(penguins)
                penguins
                path_to_file
                penguins_raw
            "#;
        let diagnostics = generate_diagnostics(code, &state);
        assert!(diagnostics.is_empty());

        let code = r#"
                penguins
                path_to_file
                penguins_raw
                library(penguins)
            "#;
        let diagnostics = generate_diagnostics(code, &state);
        assert_eq!(diagnostics.len(), 3);
    }

    #[test]
    fn test_no_diagnostics_for_library_files() {
        // Library files are foreign code the user can't fix, so we don't
        // diagnose them even though `undefined_symbol` is unresolved.
        let library = TempDir::new().unwrap();
        let pkg_dir = library.path().join("mockpkg");
        std::fs::create_dir_all(pkg_dir.join("R")).unwrap();
        std::fs::write(
            pkg_dir.join("DESCRIPTION"),
            "Package: mockpkg\nVersion: 1.0.0\n",
        )
        .unwrap();
        std::fs::write(pkg_dir.join("R").join("a.R"), "undefined_symbol\n").unwrap();
        oak_package_metadata::tests::mark_installed(&pkg_dir);

        let mut db = OakDatabase::new();
        db.set_library_paths(&[library.path().to_path_buf()]);

        let package = db.package_by_name("mockpkg").unwrap();
        // The scanner registers installed packages without reading their
        // sources; simulate the Oak cache's `set_package_sources` ingestion.
        db.set_package_sources(package, &pkg_dir.join("R"));
        let file = *package.files(&db).first().unwrap();

        let state = TestState::with_db(db);

        let url = url::Url::parse("file:///a.R").unwrap();
        let uri: Uri = url.as_str().parse().unwrap();
        let diagnostics = state.diagnose(file, &uri);
        assert!(diagnostics.is_empty());
    }

    /// The syntax error (`}`) remains when unresolved libraries suppress the
    /// out-of-scope diagnostic for `foo`.
    const SYNTAX_AND_SEMANTIC: &str = "foo\n1 }\n";

    /// [`OakDatabase::new()`] starts with the default environment resolved
    /// to an empty library list, so semantic checks run without starting R.
    fn full_diagnostics() -> Vec<lsp_types::Diagnostic> {
        generate_diagnostics(SYNTAX_AND_SEMANTIC, &TestState::default())
    }

    #[test]
    fn test_default_world_state_gives_full_diagnostics() {
        let diagnostics = full_diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].range.start, Position::new(1, 2));
        assert_eq!(diagnostics[1].message, "No symbol named 'foo' in scope.");
    }

    #[test]
    fn test_unresolved_environment_gives_syntax_diagnostics_only() {
        let mut db = OakDatabase::new();
        db.mark_default_environment_unresolved();
        let state = TestState::with_db(db);

        let diagnostics = generate_diagnostics(SYNTAX_AND_SEMANTIC, &state);
        assert_eq!(diagnostics, full_diagnostics()[..1].to_vec());
    }

    /// `base::` has no error node, so the syntax pass alone doesn't report it.
    /// The unresolved environment still drops "Package 'base' is not installed".
    #[test]
    fn test_unresolved_environment_reports_missing_namespace_rhs() {
        let mut db = OakDatabase::new();
        db.mark_default_environment_unresolved();
        let state = TestState::with_db(db);

        let messages: Vec<String> = generate_diagnostics("f(base::)\nutils::\n", &state)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(messages, vec![
            "Invalid namespace operator '::'. Missing a right hand side.",
            "Invalid namespace operator '::'. Missing a right hand side.",
        ]);
    }

    /// Without R, no startup file ran, so a workspace failure points to no
    /// startup file and gives the restart advice.
    #[test]
    fn test_missing_r_failure_gives_restart_advice_in_workspace() {
        let workspace = TempDir::new().unwrap();
        std::fs::write(workspace.path().join(".Rprofile"), "").unwrap();
        let script = workspace.path().join("a.R");
        std::fs::write(&script, SYNTAX_AND_SEMANTIC).unwrap();

        let mut db = OakDatabase::new();
        set_workspace_paths(&mut db, &[workspace.path().to_path_buf()]);
        db.reconcile_environments();

        let file_path = FilePath::from_path_buf(script.clone()).unwrap();
        let file = db.file_by_path(&file_path).unwrap();
        let environment = file.environment(&db);
        db.record_environment_failure(environment, String::from(NO_R_EXECUTABLE));
        let state = TestState::with_db(db);

        let url = url::Url::from_file_path(&script).unwrap();
        let uri: Uri = url.as_str().parse().unwrap();
        let diagnostics = state.diagnose(file, &uri);
        assert_eq!(
            diagnostics[0].message,
            format!(
                "R startup failed in {dir}, so only syntax is checked.\n\
                 \n\
                 {NO_R_EXECUTABLE}\n\
                 \n\
                 {DEFAULT_ENVIRONMENT_FIXES}",
                dir = workspace.path().display(),
            )
        );
    }

    #[test]
    fn test_first_failure_gives_syntax_and_information_until_success() {
        let mut db = OakDatabase::new();
        db.mark_default_environment_unresolved();
        let default = db.default_environment();
        db.record_environment_failure(default, String::from("no R executable was found"));
        let mut state = TestState::with_db(db);

        let diagnostics = generate_diagnostics(SYNTAX_AND_SEMANTIC, &state);
        let [failure, syntax] = diagnostics.as_slice() else {
            panic!("Expected two diagnostics, got {diagnostics:?}");
        };
        assert_eq!(
            failure.severity,
            Some(lsp_types::DiagnosticSeverity::INFORMATION)
        );
        assert_eq!(failure.range, lsp_types::Range::default());
        assert_eq!(
            failure.message,
            "R startup failed for the default environment (files outside workspace folders), \
             so only syntax is checked.\n\
             \n\
             no R executable was found\n\
             \n\
             Fixes, such as installing R or editing ~/.Rprofile, take effect after you restart R."
        );
        assert_eq!(*syntax, full_diagnostics()[0]);

        state.db.resolve_environment(default, Vec::new());
        let diagnostics = generate_diagnostics(SYNTAX_AND_SEMANTIC, &state);
        assert_eq!(diagnostics, full_diagnostics());
    }

    #[test]
    fn test_failed_rerun_gives_full_diagnostics_and_warning_until_success() {
        let mut db = OakDatabase::new();
        let default = db.default_environment();
        db.record_environment_failure(
            default,
            String::from("R exited with status 1:\nError: boom\nExecution halted"),
        );
        let mut state = TestState::with_db(db);

        let diagnostics = generate_diagnostics(SYNTAX_AND_SEMANTIC, &state);
        let [failure, rest @ ..] = diagnostics.as_slice() else {
            panic!("Expected diagnostics");
        };
        assert_eq!(
            failure.severity,
            Some(lsp_types::DiagnosticSeverity::WARNING)
        );
        assert_eq!(failure.range, lsp_types::Range::default());
        assert_eq!(
            failure.message,
            "R startup failed for the default environment (files outside workspace folders), \
             so libraries come from the last successful startup.\n\
             \n\
             R exited with status 1:\n\
             Error: boom\n\
             Execution halted\n\
             \n\
             Fixes, such as installing R or editing ~/.Rprofile, take effect after you restart R."
        );
        assert_eq!(rest.to_vec(), full_diagnostics());

        state.db.resolve_environment(default, Vec::new());
        let diagnostics = generate_diagnostics(SYNTAX_AND_SEMANTIC, &state);
        assert_eq!(diagnostics, full_diagnostics());
    }

    /// Creating or deleting startup files updates the hint without another
    /// environment resolution.
    #[test]
    fn test_workspace_failure_points_to_existing_startup_files() {
        let workspace = TempDir::new().unwrap();
        let rprofile = workspace.path().join(".Rprofile");
        let renviron = workspace.path().join(".Renviron");
        let script = workspace.path().join("a.R");
        std::fs::write(&rprofile, "stop('boom')\n").unwrap();
        std::fs::write(&script, SYNTAX_AND_SEMANTIC).unwrap();

        let mut db = OakDatabase::new();
        set_workspace_paths(&mut db, &[workspace.path().to_path_buf()]);
        db.reconcile_environments();

        let file_path = FilePath::from_path_buf(script.clone()).unwrap();
        let file = db.file_by_path(&file_path).unwrap();
        let environment = file.environment(&db);
        assert_ne!(environment, db.default_environment());
        db.record_environment_failure(
            environment,
            String::from("R exited with status 1:\nError: boom\nExecution halted"),
        );
        let state = TestState::with_db(db);

        let url = url::Url::from_file_path(&script).unwrap();
        let uri: Uri = url.as_str().parse().unwrap();
        let failure_message = |state: &TestState| {
            let diagnostics = state.diagnose(file, &uri);
            assert_eq!(diagnostics.len(), 2);
            diagnostics[0].message.clone()
        };
        let dir = workspace.path().display();
        let message = |see: &str| {
            format!(
                "R startup failed in {dir}, so only syntax is checked.{see}\n\
                 \n\
                 R exited with status 1:\n\
                 Error: boom\n\
                 Execution halted\n\
                 \n\
                 Edits to .Rprofile or .Renviron in this directory are picked up automatically. \
                 Other fixes, such as installing R or renv, or editing ~/.Rprofile, take effect after you restart R."
            )
        };

        let rprofile_shown = rprofile.display();
        let renviron_shown = renviron.display();
        assert_eq!(
            failure_message(&state),
            message(&format!(" See {rprofile_shown}."))
        );

        std::fs::write(&renviron, "").unwrap();
        assert_eq!(
            failure_message(&state),
            message(&format!(" See {rprofile_shown} and {renviron_shown}."))
        );

        std::fs::remove_file(&rprofile).unwrap();
        std::fs::remove_file(&renviron).unwrap();
        assert_eq!(failure_message(&state), message(""));
    }
}
