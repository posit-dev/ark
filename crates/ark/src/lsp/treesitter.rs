//
// treesitter.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::external_ptr::ExternalPointer;
use harp::object::RObject;
use libR_sys::SEXP;
use libR_sys::*;
use tree_sitter::Node;

use crate::lsp::diagnostics::DiagnosticContext;

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_text(node_ptr: SEXP, context_ptr: SEXP) -> SEXP {
    let node = ExternalPointer::<Node>::reference(node_ptr);
    let context = ExternalPointer::<DiagnosticContext>::reference(context_ptr);

    let text = node.utf8_text(context.source.as_bytes()).unwrap_or("");
    *RObject::from(text)
}

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_kind(node_ptr: SEXP) -> SEXP {
    let node = ExternalPointer::<Node>::reference(node_ptr);

    *RObject::from(node.kind())
}
