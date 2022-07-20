// 
// completions.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::collections::HashSet;
use std::ffi::CStr;

use extendr_api::Robj;
use extendr_api::Strings;
use libR_sys::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::CompletionParams;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::traits::node::NodeExt;
use crate::macros::*;
use crate::lsp::document::Document;
use crate::lsp::logger::dlog;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::position::PositionExt;
use crate::r::lock::rlock;
use crate::r::macros::install;
use crate::r::exec::RFunction;
use crate::r::exec::RFunctionExt;
use crate::r::exec::RProtect;

fn completion_from_identifier(node: &Node, source: &str) -> CompletionItem {
    let label = node.utf8_text(source.as_bytes()).expect("empty assignee");
    let detail = format!("Defined on line {}", node.start_position().row + 1);
    CompletionItem::new_simple(label.to_string(), detail)
}

struct CompletionData {
    source: String,
    position: Point,
    visited: HashSet<usize>,
}

fn call_uses_nse(node: &Node, data: &CompletionData) -> bool {

    // get the callee
    let lhs = unwrap!(node.child(0), {
        return false;
    });

    // validate we have an identifier or a string
    match lhs.kind() {
        "identifier" | "string" => {},
        _ => { return false; }
    }

    // check for a function whose evaluation occurs in a local scope
    let value = unwrap!(lhs.utf8_text(data.source.as_bytes()), {
        return false;
    });

    match value {
        "expression" | "local" | "quote" | "enquote" | "substitute" | "with" | "within" => { return true; },
        _ => { return false; }
    }

}

fn append_defined_variables(node: &Node, data: &mut CompletionData, completions: &mut Vec<CompletionItem>) {

    let mut cursor = node.walk();
    cursor.recurse(|node| {

        // skip nodes that exist beyond the completion position
        if node.start_position().is_after(data.position) {
            return false;
        }

        // skip nodes that were already visited
        if data.visited.contains(&node.id()) {
            return false;
        }

        match node.kind() {

            "left_assignment" | "super_assignment" | "equals_assignment" => {

                // TODO: Should we de-quote symbols and strings, or insert them as-is?
                let assignee = node.child(0).unwrap();
                if assignee.kind() == "identifier" || assignee.kind() == "string" {
                    completions.push(completion_from_identifier(&assignee, &data.source));
                }

                // return true in case we have nested assignments
                return true;

            }

            "right_assignment" | "super_right_assignment" => {

                // return true for nested assignments
                return true;

            }

            "call" => {

                // don't recurse into calls for certain functions
                return !call_uses_nse(&node, &data);

            }

            "function_definition" => {

                // don't recurse into function definitions, as these create as new scope
                // for variable definitions (and so such definitions are no longer visible)
                return false;

            }

            _ => {
                return true;
            }

        }

    });

}

fn append_function_parameters(node: &Node, data: &mut CompletionData, completions: &mut Vec<CompletionItem>) {

    let mut cursor = node.walk();
    
    if !cursor.goto_first_child() {
        dlog!("goto_first_child() failed");
        return;
    }

    if !cursor.goto_next_sibling() {
        dlog!("goto_next_sibling() failed");
        return;
    }

    let kind = cursor.node().kind();
    if kind != "formal_parameters" {
        dlog!("unexpected node kind {}", kind);
        return;
    }

    if !cursor.goto_first_child() {
        dlog!("goto_first_child() failed");
        return;
    }

    // The R tree-sitter grammar doesn't parse an R function's formals list into
    // a tree; instead, it's just held as a sequence of tokens. that said, the
    // only way an identifier could / should show up here is if it is indeed a
    // function parameter, so just search direct children here for identifiers.
    while cursor.goto_next_sibling() {
        let node = cursor.node();
        if node.kind() == "identifier" {
            completions.push(completion_from_identifier(&node, data.source.as_str()));
        }
    }

}

fn list_namespace_symbols(namespace: SEXP, exports_only: bool, protect: &mut RProtect) -> SEXP { unsafe {

    if !exports_only {
        return protect.add(R_lsInternal(namespace, 1));
    }

    let ns = Rf_findVarInFrame(namespace, install!(".__NAMESPACE__."));
    if ns == R_UnboundValue {
        return R_NilValue;
    }

    let exports = Rf_findVarInFrame(ns, install!("exports"));
    if exports == R_UnboundValue {
        return R_NilValue;
    }

    return protect.add(R_lsInternal(exports, 1));

} }

fn append_parameter_completions(callee: &str, completions: &mut Vec<CompletionItem>) { rlock! {

    dlog!("append_parameter_completions({:?})", callee);

    // TODO: Given the callee, we should also try to find its definition within
    // the document index of function definitions, since it may not be defined
    // within the session.
    let mut protect = RProtect::new();
    let mut status: ParseStatus = 0;

    // Parse the callee text. The text will be parsed as an R expression,
    // which is a vector of calls to be evaluated.
    let string_sexp = protect.add(Rf_allocVector(STRSXP, 1));
    SET_STRING_ELT(string_sexp, 0, Rf_mkCharLenCE(callee.as_ptr() as *const i8, callee.len() as i32, cetype_t_CE_UTF8));
    let parsed_sexp = protect.add(R_ParseVector(string_sexp, 1, &mut status, R_NilValue));

    if status != ParseStatus_PARSE_OK {
        dlog!("Error parsing {} [status {}]", callee, status);
        return;
    }

    // Evaluate the text.
    let mut value = R_NilValue;
    for i in 0..Rf_length(parsed_sexp) {
        let expr = VECTOR_ELT(parsed_sexp, i as isize);
        value = Rf_eval(expr, R_GlobalEnv);
    }
    
    if Rf_isFunction(value) != 0 {

        // For primitive functions, we use the 'args()' function to get
        // a function with a compatible prototype.
        if Rf_isPrimitive(value) != 0 {
            value = RFunction::new("base", "args")
                .add(value)
                .call(&mut protect);
        }

        // Now, we can use 'names(formals())' to get the names of
        // the function's formal arguments.
        let formals = RFunction::new("base", "formals")
            .add(value)
            .call(&mut protect);

        let names = RFunction::new("base", "names")
            .add(formals)
            .call(&mut protect);

        // Return the names of these formals.
        let names = Robj::from_sexp(names);
        let strings = Strings::try_from(names).unwrap();
        for string in strings.iter() {
            let mut item = CompletionItem::new_simple(string.to_string(), callee.to_string());
            item.kind = Some(CompletionItemKind::FIELD);
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            item.insert_text = Some(string.to_string() + " = ");

            item.detail = Some("This is some detail.".to_string());
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "# This is a Markdown header.".to_string(),
            }));

            completions.push(item);
        }

    }

} }

fn append_namespace_completions(package: &str, exports_only: bool, completions: &mut Vec<CompletionItem>) { rlock! {

    dlog!("append_namespace_completions({:?}, {})", package, exports_only);
    let mut protect = RProtect::new();

    // Get the package namespace.
    let namespace = RFunction::new("base", "getNamespace")
        .add(package)
        .call(&mut protect);

    let symbols = list_namespace_symbols(namespace, exports_only, &mut protect);

    if TYPEOF(symbols) as u32 != STRSXP {
        dlog!("Unexpected SEXPTYPE {}", TYPEOF(symbols));
        return;
    }

    for i in 0..Rf_length(symbols) {

        // Get a reference to the underlying C string.
        let ptr = R_CHAR(STRING_ELT(symbols, i as isize));
        let cstr = CStr::from_ptr(ptr);

        // Start building the completion item data.
        let label = cstr.to_str().unwrap();

        // 'detail' is the label displayed in the 'title bar' of the
        // associated Help popup. We'll display the associated function
        // signature there.
        let value = RFunction::new("base", "get")
            .param("x", label)
            .param("envir", namespace)
            .call(&mut protect);
        Rf_PrintValue(value);

        let wrapper = RFunction::new("base", "args")
            .add(value)
            .call(&mut protect);

        let formatted = RFunction::new("base", "format")
            .add(wrapper)
            .call(&mut protect);

        let mut detail = String::new();
        for i in 0..(Rf_length(formatted) - 1) {
            let ptr = R_CHAR(STRING_ELT(symbols, i as isize));
            let cstr = CStr::from_ptr(ptr);
            detail.push_str(cstr.to_str().unwrap());
        }

        // Create the completion item.
        let mut item = CompletionItem::new_simple(format!("{}()", label), detail);

        // Use a 'snippet' for insertion, so that parentheses are
        // automatically appended with the cursor moved in-between
        // the provided parentheses.
        item.kind = Some(CompletionItemKind::FUNCTION);
        item.insert_text = Some(format!("{}($0)", label));
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);

        completions.push(item);
    }

} }

#[allow(dead_code)]
fn append_keyword_completions(completions: &mut Vec<CompletionItem>) {

    let keywords = vec![
        "NULL", "NA", "TRUE", "FALSE", "Inf", "NaN", "NA_integer_",
        "NA_real_", "NA_character_", "NA_complex_", "function", "while",
        "repeat", "for", "if", "in", "else", "next", "break", "return",
    ];

    for keyword in keywords {
        let mut item = CompletionItem::new_simple(keyword.to_string(), "[keyword]".to_string());
        item.kind = Some(CompletionItemKind::KEYWORD);
        completions.push(item);
    }

}

pub(crate) fn append_session_completions(document: &mut Document, params: &CompletionParams, completions: &mut Vec<CompletionItem>) {

    // get reference to AST
    let ast = unwrap!(document.ast.as_ref(), {
        return;
    });

    // figure out the token / node at the cursor position. note that we use
    // the previous token here as the cursor itself will be located just past
    // the cursor / node providing the associated context
    let point = params.text_document_position.position.as_point();
    let point = Point::new(point.row, point.column - 1);
    let mut node = unwrap!(ast.root_node().descendant_for_point_range(point, point), {
        return;
    });

    // check to see if we're completing a symbol from a namespace,
    // via code like:
    //
    //   package::sym
    //   package:::sym
    //
    // note that we'll need to handle cases where the user hasn't
    // yet started typing the symbol name, so that the cursor would
    // be on the '::' or ':::' token.
    //
    // Note that treesitter collects the tokens into a tree of the form:
    //
    //    - stats::bar - namespace_get
    //    - stats - identifier
    //    - :: - ::
    //    - bar - identifier
    //
    // But, if the tree is not yet complete, then treesitter gives us:
    //
    //    - stats - identifier
    //    - :: - ERROR
    //      - :: - ::
    //
    // So we have to do some extra work to get the package name in each case.
    dlog!("Completion from node: {:?}", node);

    let source = document.contents.to_string();
    let dump = ast.root_node().dump(&source);
    dlog!("AST: {}", dump);

    // Handle the case with 'package::', with no identifier name yet following.
    if matches!(node.kind(), "::" | ":::") {
        let exports_only = node.kind() == "::";
        if let Some(parent) = node.parent() {
            if parent.kind() == "ERROR" {
                if let Some(prev) = parent.prev_sibling() {
                    if matches!(prev.kind(), "identifier" | "string") {
                        let package = prev.utf8_text(source.as_bytes()).unwrap();
                        append_namespace_completions(package, exports_only, completions);
                    }
                }
            }
        }
    }

    loop {

        // If we landed on a 'call', then we should provide parameter completions
        // for the associated callee if possible.
        if node.kind() == "call" {
            if let Some(child) = node.child(0) {
                let text = child.utf8_text(source.as_bytes()).unwrap();
                append_parameter_completions(&text, completions);
                break;
            };
        }

        // Handle the case with 'package::prefix', where the user has now
        // started typing the prefix of the symbol they would like completions for.
        if matches!(node.kind(), "namespace_get" | "namespace_get_internal") {
            if let Some(package_node) = node.child(0) {
                if let Some(colon_node) = node.child(1) {
                    let package = package_node.utf8_text(source.as_bytes()).unwrap();
                    let exports_only = colon_node.kind() == "::";
                    append_namespace_completions(package, exports_only, completions);
                    break;
                }
            }
        }

        // If we reach a brace list, bail.
        if node.kind() == "brace_list" {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break
        };

    }

}

pub(crate) fn append_document_completions(document: &mut Document, params: &CompletionParams, completions: &mut Vec<CompletionItem>) {

    // get reference to AST
    let ast = unwrap!(document.ast.as_ref(), {
        return;
    });

    // try to find child for point
    let point = params.text_document_position.position.as_point();
    let mut node = unwrap!(ast.root_node().descendant_for_point_range(point, point), {
        return;
    });

    // build completion data
    let mut data = CompletionData {
        source: document.contents.to_string(),
        position: point,
        visited: HashSet::new(),
    };

    loop {

        // If this is a brace list, or the document root, recurse to find identifiers.
        if node.kind() == "brace_list" || node.parent() == None {
            append_defined_variables(&node, &mut data, completions);
        }

        // If this is a function definition, add parameter names.
        if node.kind() == "function_definition" {
            append_function_parameters(&node, &mut data, completions);
        }

        // Mark this node as visited.
        data.visited.insert(node.id());

        // Keep going.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };

    }

}
