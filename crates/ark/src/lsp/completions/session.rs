//
// session.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::env::current_dir;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::string::r_string_decode;
use harp::utils::r_env_is_pkg_env;
use harp::utils::r_envir_name;
use harp::utils::r_normalize_path;
use harp::utils::r_typeof;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::*;
use log::*;
use regex::Regex;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use tree_sitter::Node;
use yaml_rust::YamlLoader;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::completions::completion_item::completion_item_from_dataset;
use crate::lsp::completions::completion_item::completion_item_from_direntry;
use crate::lsp::completions::completion_item::completion_item_from_lazydata;
use crate::lsp::completions::completion_item::completion_item_from_namespace;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::completion_item::completion_item_from_symbol;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::completions::types::PromiseStrategy;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;
use crate::lsp::signature_help::signature_help;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::tree::TreeExt;

pub(super) unsafe fn append_session_completions(
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    info!("append_session_completions()");

    // get reference to AST
    let cursor = context.node;
    let mut node = cursor;

    // check for completion within a comment -- in such a case, we usually
    // want to complete things like roxygen tags
    //
    // TODO: should some of this token processing happen in treesitter?
    if node.kind() == "comment" {
        let pattern = Regex::new(r"^.*\s").unwrap();
        let contents = node.utf8_text(context.source.as_bytes()).unwrap();
        let token = pattern.replace(contents, "");
        if token.starts_with('@') {
            return append_roxygen_completions(&token[1..], completions);
        } else {
            return Ok(());
        }
    }

    let mut use_search_path = true;
    let mut found_call_completions = false;

    loop {
        // Check for 'subset' completions.
        if matches!(node.kind(), "$" | "[" | "[[") {
            use_search_path = false;
            let enquote = matches!(node.kind(), "[" | "[[");
            if let Some(child) = node.child(0) {
                let text = child.utf8_text(context.source.as_bytes())?;
                unwrap!(
                    append_subset_completions(context, &text, enquote, completions),
                    Err(error) => {
                        log::error!("{}", error);
                    }
                );
            }
        }

        // If we landed on a 'call', then we should provide parameter completions
        // for the associated callee if possible.
        if !found_call_completions && node.kind() == "call" {
            found_call_completions = true;

            // Check for library() completions.
            match append_custom_completions(context, completions) {
                Ok(done) => {
                    if done {
                        return Ok(());
                    }
                },
                Err(error) => error!("{}", error),
            }

            // Check for pipe completions.
            if let Err(error) = append_pipe_completions(context, &node, completions) {
                log::error!("{}", error);
            }

            // Check for generic call completions.
            if let Err(error) = append_call_completions(context, &cursor, &node, completions) {
                log::error!("{}", error);
            }
        }

        // Handle the case with 'package::prefix', where the user has now
        // started typing the prefix of the symbol they would like completions for.
        if matches!(node.kind(), "::" | ":::") {
            let exports_only = node.kind() == "::";
            if let Some(node) = node.child(0) {
                let package = node.utf8_text(context.source.as_bytes())?;
                append_namespace_completions(context, package, exports_only, completions)?;
                use_search_path = false;
                break;
            }
        }

        // If we reach a brace list, bail.
        if node.kind() == "{" {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    // If we get here, and we were located within a string,
    // just provide file completions.
    if cursor.kind() == "string" {
        return append_file_completions(context, completions);
    }

    // If we got here, then it's appropriate to return completions
    // for any packages + symbols on the search path.
    if use_search_path {
        append_search_path_completions(context, completions)?;
    }

    Ok(())
}

unsafe fn append_roxygen_completions(
    _token: &str,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // TODO: cache these?
    // TODO: use an indexer to build the tag list?
    let tags = RFunction::new("base", "system.file")
        .param("package", "roxygen2")
        .add("roxygen2-tags.yml")
        .call()?
        .to::<String>()?;

    if tags.is_empty() {
        return Ok(());
    }

    let tags = Path::new(&tags);
    if !tags.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(tags).unwrap();
    let docs = YamlLoader::load_from_str(contents.as_str()).unwrap();
    let doc = &docs[0];

    let items = doc.as_vec().unwrap();
    for entry in items.iter() {
        let name = unwrap!(entry["name"].as_str(), None => {
            continue;
        });

        let label = name.to_string();
        let mut item = completion_item(label.clone(), CompletionData::RoxygenTag {
            tag: label.clone(),
        })?;

        // TODO: What is the appropriate icon for us to use here?
        let template = entry["template"].as_str();
        if let Some(template) = template {
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            item.insert_text = Some(format!("{}{}", name, template));
        } else {
            item.insert_text = Some(format!("@{}", label.as_str()));
        }

        item.detail = Some(format!("roxygen @{} (R)", name));
        if let Some(description) = entry["description"].as_str() {
            let markup = MarkupContent {
                kind: MarkupKind::Markdown,
                value: description.to_string(),
            };
            item.documentation = Some(Documentation::MarkupContent(markup));
        }

        completions.push(item);
    }

    return Ok(());
}

unsafe fn append_subset_completions(
    _context: &DocumentContext,
    callee: &str,
    enquote: bool,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    info!("append_subset_completions({:?})", callee);

    let value = r_parse_eval(callee, RParseEvalOptions {
        forbid_function_calls: true,
    })?;

    let names = RFunction::new("base", "names")
        .add(value)
        .call()?
        .to::<Vec<String>>()?;

    for name in names {
        match completion_item_from_data_variable(&name, callee, enquote) {
            Ok(item) => completions.push(item),
            Err(error) => error!("{:?}", error),
        }
    }

    Ok(())
}

unsafe fn append_pipe_completions(
    context: &DocumentContext,
    node: &Node,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // Try to figure out the code associated with the 'root' of the pipe expression.
    let root = local! {

        let root = find_pipe_root(*node)?;
        is_pipe_operator(&root).into_option()?;

        // Get the left-hand side of the pipe expression.
        let mut lhs = root.child_by_field_name("lhs")?;
        while is_pipe_operator(&lhs) {
            lhs = lhs.child_by_field_name("lhs")?;
        }

        // Try to evaluate the left-hand side
        let root = lhs.utf8_text(context.source.as_bytes()).ok()?;
        Some(root)

    };

    let root = unwrap!(root, None => {
        return Ok(());
    });

    let value = r_parse_eval(root, RParseEvalOptions {
        forbid_function_calls: true,
    })?;

    // Try to retrieve names from the resulting item
    let names = RFunction::new("base", "names")
        .add(value)
        .call()?
        .to::<Vec<String>>()?;

    for name in names {
        let item = completion_item_from_data_variable(&name, root, false)?;
        completions.push(item);
    }

    Ok(())
}

fn find_pipe_root(mut node: Node) -> Option<Node> {
    let mut root = None;

    loop {
        if is_pipe_operator(&node) {
            root = Some(node);
        }

        node = match node.parent() {
            Some(node) => node,
            None => return root,
        }
    }
}

fn is_pipe_operator(node: &Node) -> bool {
    matches!(node.kind(), "%>%" | "|>")
}

unsafe fn append_custom_completions(
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<bool> {
    // Use the signature help tools to figure out the necessary pieces.
    let position = context.point.as_position();
    let signatures = signature_help(context.document, &position)?;

    let signatures = unwrap!(signatures, None => {
        return Ok(false);
    });

    // Pull out the relevant signature information.
    let signature = signatures.signatures.get(0).into_result()?;
    let mut name = signature.label.clone();
    let parameters = signature.parameters.as_ref().into_result()?;
    let index = signature.active_parameter.into_result()?;
    let parameter = parameters.get(index as usize).into_result()?;

    // Extract the argument text.
    let argument = match parameter.label.clone() {
        tower_lsp::lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
            let label = signature.label.as_str();
            let substring = label.get((start as usize)..(end as usize));
            substring.unwrap().to_string()
        },
        tower_lsp::lsp_types::ParameterLabel::Simple(string) => string,
    };

    // Trim off the function arguments from the signature.
    if let Some(index) = name.find('(') {
        name = name[0..index].to_string();
    }

    // Check and see if we're in the 'name' position,
    // versus the 'value' position, for a function invocation.
    //
    // For example:
    //
    //    Sys.setenv(EDITOR = "vim")
    //               ^^^^^^   ^^^^^
    //                name    value
    //
    // This is mainly relevant because we might only want to
    // provide certain completions in the 'name' position.
    let node = context.document.ast.node_at_point(context.point);

    let marker = node.bwd_leaf_iter().find_map(|node| match node.kind() {
        "(" | "comma" => Some("name"),
        "=" => Some("value"),
        _ => None,
    });

    let position = marker.unwrap_or("value");

    // Call our custom completion function.
    let r_completions = RFunction::from(".ps.completions.getCustomCallCompletions")
        .param("name", name)
        .param("argument", argument)
        .param("position", position)
        .call()?;

    if r_typeof(*r_completions) != VECSXP {
        return Ok(false);
    }

    // TODO: Use safe access APIs here.
    let values = VECTOR_ELT(*r_completions, 0);
    let kind = VECTOR_ELT(*r_completions, 1);
    let enquote = VECTOR_ELT(*r_completions, 2);
    let append = VECTOR_ELT(*r_completions, 3);
    if let Ok(values) = RObject::view(values).to::<Vec<String>>() {
        let kind = RObject::view(kind)
            .to::<String>()
            .unwrap_or("unknown".to_string());
        let enquote = RObject::view(enquote).to::<bool>().unwrap_or(false);
        let append = RObject::view(append)
            .to::<String>()
            .unwrap_or("".to_string());
        for value in values.iter() {
            let value = value.clone();
            let item = match kind.as_str() {
                "package" => completion_item_from_package(&value, false),
                "dataset" => completion_item_from_dataset(&value),
                _ => completion_item(&value, CompletionData::Unknown),
            };

            let mut item = unwrap!(item, Err(error) => {
                log::error!("{}", error);
                continue;
            });

            if enquote && node.kind() != "string" {
                item.insert_text = Some(format!("\"{}\"", value));
            } else if !append.is_empty() {
                item.insert_text = Some(format!("{}{}", value, append));
            }

            completions.push(item);
        }
    }

    Ok(true)
}

unsafe fn append_call_completions(
    context: &DocumentContext,
    _cursor: &Node,
    node: &Node,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // Get the caller text.
    let callee = node.child(0).into_result()?;
    let callee = callee.utf8_text(context.source.as_bytes())?;

    // Get the first argument, if any (object used for dispatch).
    // TODO: We should have some way of matching calls, so we can
    // take a function signature from R and see how the call matches
    // to that object.
    let mut object: Option<&str> = None;
    if let Some(arguments) = node.child_by_field_name("arguments") {
        let mut cursor = arguments.walk();
        let mut children = arguments.children_by_field_name("argument", &mut cursor);
        if let Some(argument) = children.next() {
            if let None = argument.child_by_field_name("name") {
                if let Some(value) = argument.child_by_field_name("value") {
                    let text = value.utf8_text(context.source.as_bytes())?;
                    object = Some(text);
                }
            }
        }
    }

    append_argument_completions(context, &callee, object, completions)
}

unsafe fn append_argument_completions(
    _context: &DocumentContext,
    callable: &str,
    object: Option<&str>,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    info!("append_argument_completions({:?})", callable);

    // Check for a function defined in the workspace that can provide parameters.
    if let Some((_path, entry)) = indexer::find(callable) {
        #[allow(unused)]
        match entry.data {
            indexer::IndexEntryData::Function { name, arguments } => {
                for argument in arguments {
                    match completion_item_from_parameter(argument.as_str(), name.as_str()) {
                        Ok(item) => completions.push(item),
                        Err(error) => error!("{:?}", error),
                    }
                }
            },

            indexer::IndexEntryData::Section { level, title } => {
                // nothing to do
            },
        }
    }

    // Otherwise, try to retrieve completion names from the object itself.
    let r_callable = r_parse_eval(callable, RParseEvalOptions {
        forbid_function_calls: true,
    })?;

    // If the user is writing pseudocode, this object might not exist yet,
    // in which case we just want to ignore the error from trying to evaluate it
    // and just provide typical completions.
    let r_object = if let Some(object) = object {
        let options = RParseEvalOptions {
            forbid_function_calls: true,
        };
        r_parse_eval(object, options).unwrap_or_else(|error| {
            log::info!("append_argument_completions(): Failed to evaluate first argument: {error}");
            RObject::null()
        })
    } else {
        RObject::null()
    };

    let strings = RFunction::from(".ps.completions.formalNames")
        .add(r_callable)
        .add(r_object)
        .call()?
        .to::<Vec<String>>()?;

    // Return the names of these formals.
    for string in strings.iter() {
        match completion_item_from_parameter(string, callable) {
            Ok(item) => completions.push(item),
            Err(error) => error!("{:?}", error),
        }
    }

    Ok(())
}

unsafe fn append_namespace_completions(
    _context: &DocumentContext,
    package: &str,
    exports_only: bool,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    info!(
        "append_namespace_completions({:?}, {})",
        package, exports_only
    );

    // Get the package namespace.
    let namespace = RFunction::new("base", "getNamespace").add(package).call()?;

    let symbols = if package == "base" {
        list_namespace_symbols(*namespace)
    } else if exports_only {
        list_namespace_exports(*namespace)
    } else {
        list_namespace_symbols(*namespace)
    };

    let strings = symbols.to::<Vec<String>>()?;
    for string in strings.iter() {
        match completion_item_from_namespace(string, *namespace, package) {
            Ok(item) => completions.push(item),
            Err(error) => error!("{:?}", error),
        }
    }

    if exports_only {
        // `pkg:::object` doesn't return lazy objects, so we don't want
        // to show lazydata completions if we are inside `:::`
        append_namespace_lazydata_completions(*namespace, package, completions)?;
    }

    Ok(())
}

unsafe fn append_namespace_lazydata_completions(
    namespace: SEXP,
    package: &str,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    let ns = Rf_findVarInFrame(namespace, r_symbol!(".__NAMESPACE__."));
    if ns == R_UnboundValue {
        return Ok(());
    }

    let env = Rf_findVarInFrame(ns, r_symbol!("lazydata"));
    if env == R_UnboundValue {
        return Ok(());
    }

    let names = RObject::to::<Vec<String>>(RObject::from(R_lsInternal(env, Rboolean_TRUE)))?;

    for name in names.iter() {
        match completion_item_from_lazydata(name, env, package) {
            Ok(item) => completions.push(item),
            Err(error) => error!("{:?}", error),
        }
    }

    Ok(())
}

unsafe fn list_namespace_exports(namespace: SEXP) -> RObject {
    let ns = Rf_findVarInFrame(namespace, r_symbol!(".__NAMESPACE__."));
    if ns == R_UnboundValue {
        return RObject::null();
    }

    let exports = Rf_findVarInFrame(ns, r_symbol!("exports"));
    if exports == R_UnboundValue {
        return RObject::null();
    }

    return RObject::new(R_lsInternal(exports, 1));
}

unsafe fn list_namespace_symbols(namespace: SEXP) -> RObject {
    return RObject::new(R_lsInternal(namespace, 1));
}

unsafe fn append_search_path_completions(
    _context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    const R_CONTROL_FLOW_KEYWORDS: &[&str] = &[
        "if", "else", "for", "in", "while", "repeat", "break", "next", "return", "function",
    ];

    // Start with keyword completions.
    append_keyword_completions(completions)?;

    // Iterate through environments starting from the global environment.
    let mut envir = R_GlobalEnv;

    while envir != R_EmptyEnv {
        // Get environment name
        let name = r_envir_name(envir)?;

        // If this is a package environment, we will need to force promises to give meaningful completions,
        // particularly with functions because we add a `CompletionItem::command()` that adds trailing `()` onto
        // the completion and triggers parameter completions.
        let promise_strategy = if r_env_is_pkg_env(envir) {
            PromiseStrategy::Force
        } else {
            PromiseStrategy::Simple
        };

        // List symbols in the environment.
        let symbols = R_lsInternal(envir, 1);

        // Create completion items for each.
        let vector = CharacterVector::new(symbols)?;
        for symbol in vector.iter() {
            // Skip missing values.
            let Some(symbol) = symbol else {
                continue;
            };

            // Skip control flow keywords.
            let symbol = symbol.as_str();
            if R_CONTROL_FLOW_KEYWORDS.contains(&symbol) {
                continue;
            }

            // Add the completion item.
            let Some(item) = completion_item_from_symbol(
                symbol,
                envir,
                Some(name.as_str()),
                promise_strategy.clone(),
            ) else {
                error!("Completion symbol '{symbol}' was unexpectedly not found.");
                continue;
            };

            match item {
                Ok(item) => completions.push(item),
                Err(error) => error!("{:?}", error),
            };
        }

        // Get the next environment.
        envir = ENCLOS(envir);
    }

    // Include installed packages as well.
    // TODO: This can be slow on NFS.
    let packages = RFunction::new("base", ".packages")
        .param("all.available", true)
        .call()?;

    let strings = packages.to::<Vec<String>>()?;
    for string in strings.iter() {
        let item = completion_item_from_package(string, true)?;
        completions.push(item);
    }

    Ok(())
}

fn append_keyword_completions(completions: &mut Vec<CompletionItem>) -> anyhow::Result<()> {
    // provide keyword completion results
    // NOTE: Some R keywords have definitions provided in the R
    // base namespace, so we don't need to provide duplicate
    // definitions for these here.
    let keywords = vec![
        "NULL",
        "NA",
        "TRUE",
        "FALSE",
        "Inf",
        "NaN",
        "NA_integer_",
        "NA_real_",
        "NA_character_",
        "NA_complex_",
        "in",
        "else",
        "next",
        "break",
    ];

    for keyword in keywords {
        let mut item = CompletionItem::new_simple(keyword.to_string(), "[keyword]".to_string());
        item.kind = Some(CompletionItemKind::KEYWORD);
        completions.push(item);
    }

    Ok(())
}

unsafe fn append_file_completions(
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // Get the contents of the string token.
    //
    // NOTE: This includes the quotation characters on the string, and so
    // also includes any internal escapes! We need to decode the R string
    // before searching the path entries.
    let token = context.node.utf8_text(context.source.as_bytes())?;
    let contents = r_string_decode(token).into_result()?;
    log::info!("String value (decoded): {}", contents);

    // Use R to normalize the path.
    let path = r_normalize_path(RObject::from(contents))?;

    // parse the file path and get the directory component
    let mut path = PathBuf::from(path.as_str());
    log::info!("Normalized path: {}", path.display());

    // if this path doesn't have a root, add it on
    if !path.has_root() {
        let root = current_dir()?;
        path = root.join(path);
    }

    // if this isn't a directory, get the parent path
    if !path.is_dir() {
        if let Some(parent) = path.parent() {
            path = parent.to_path_buf();
        }
    }

    // look for files in this directory
    log::info!("Reading directory: {}", path.display());
    let entries = std::fs::read_dir(path)?;
    for entry in entries.into_iter() {
        let entry = unwrap!(entry, Err(error) => {
            log::error!("{}", error);
            continue;
        });

        let item = unwrap!(completion_item_from_direntry(entry), Err(error) => {
            log::error!("{}", error);
            continue;
        });

        completions.push(item);
    }

    Ok(())
}
