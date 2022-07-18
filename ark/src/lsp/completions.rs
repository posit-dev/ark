// 
// completions.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::collections::HashSet;
use std::ffi::CStr;

use libR_sys::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionParams;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::traits::node::NodeExt;
use crate::macros::*;
use crate::lsp::document::Document;
use crate::lsp::logger::log_push;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::position::PositionExt;
use crate::r_exec::RFunction;
use crate::r_exec::RFunctionExt;
use crate::r_exec::RProtect;
use crate::r_exec::install;
use crate::r_lock::rlock;

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
        log_push!("goto_first_child() failed");
        return;
    }

    if !cursor.goto_next_sibling() {
        log_push!("goto_next_sibling() failed");
        return;
    }

    let kind = cursor.node().kind();
    if kind != "formal_parameters" {
        log_push!("unexpected node kind {}", kind);
        return;
    }

    if !cursor.goto_first_child() {
        log_push!("goto_first_child() failed");
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

    let ns = Rf_findVarInFrame(namespace, install(".__NAMESPACE__."));
    if ns == R_UnboundValue {
        return R_NilValue;
    }

    let exports = Rf_findVarInFrame(ns, install("exports"));
    if exports == R_UnboundValue {
        return R_NilValue;
    }

    return protect.add(R_lsInternal(exports, 1));

} }

fn append_namespace_completions(package: &str, exports_only: bool, completions: &mut Vec<CompletionItem>) {

    rlock! {

        let mut protect = RProtect::new();

        // Get the package namespace.
        let namespace = RFunction::new("base:::getNamespace")
            .add(package)
            .call(&mut protect);

        let symbols = list_namespace_symbols(namespace, exports_only, &mut protect);

        // Iterate over the various strings.
        if TYPEOF(symbols) as u32 == STRSXP {
            for i in 1..Rf_length(symbols) {
                let ptr = R_CHAR(STRING_ELT(symbols, i as isize));
                let cstr = CStr::from_ptr(ptr);
                let item = CompletionItem::new_simple(cstr.to_str().unwrap().to_string(), package.to_string());
                completions.push(item);
            }
        }

    }

}

pub(crate) fn append_session_completions(document: &mut Document, params: &CompletionParams, completions: &mut Vec<CompletionItem>) {

    // get reference to AST
    let ast = unwrap!(document.ast.as_ref(), {
        return;
    });

    let source = document.contents.to_string();

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
    let source = document.contents.to_string();

    // Handle the case with 'package::', with no identifier name yet following.
    if node.kind() == "::" || node.kind() == ":::" {
        let exports_only = node.kind() == "::";
        if let Some(parent) = node.parent() {
            if parent.kind() == "ERROR" {
                if let Some(prev) = parent.prev_sibling() {
                    if prev.kind() == "identifier" || prev.kind() == "string" {
                        let package = prev.utf8_text(source.as_bytes()).unwrap();
                        append_namespace_completions(package, exports_only, completions);
                    }
                }
            }
        }
    }

    // Handle the case with 'package::prefix', where the user has now
    // started typing the prefix of the symbol they would like completions for.
    if let Some(parent) = node.parent() {
        if matches!(parent.kind(), "namespace_get" | "namespace_get_internal") {
            if let Some(package_node) = parent.child(0) {
                if let Some(colon_node) = parent.child(1) {
                    let package = package_node.utf8_text(source.as_bytes()).unwrap();
                    let exports_only = colon_node.kind() == "::";
                    append_namespace_completions(package, exports_only, completions);
                }
            }
        }
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
