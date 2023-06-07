//
// treesitter.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::object::RObject;
use libR_sys::SEXP;
use libR_sys::*;
use tree_sitter::Node;

use crate::lsp::diagnostics::get_external_ptr;
use crate::lsp::diagnostics::DiagnosticContext;

#[harp::register]
pub unsafe extern "C" fn ps_diagnostics_treesitter_text(node_ptr: SEXP, context_ptr: SEXP) -> SEXP {
    let node = get_external_ptr::<Node>(node_ptr);
    let context = get_external_ptr::<DiagnosticContext>(context_ptr);

    let text = node.utf8_text(context.source.as_bytes()).unwrap_or("");
    *RObject::from(text)
}

#[harp::register]
pub unsafe extern "C" fn ps_diagnostics_treesitter_kind(node_ptr: SEXP) -> SEXP {
    let node = get_external_ptr::<Node>(node_ptr);

    *RObject::from(node.kind())
}
