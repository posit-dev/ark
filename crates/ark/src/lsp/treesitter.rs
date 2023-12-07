//
// treesitter.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use harp::external_ptr::ExternalPointer;
use harp::object::RObject;
use libR_shim::RAW;
use libR_shim::SEXP;
use tree_sitter::Node;

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_text(
    node_ptr: SEXP,
    source_ptr: SEXP,
) -> anyhow::Result<SEXP> {
    let node: Node<'static> = *(RAW(node_ptr) as *mut Node<'static>);
    let source = ExternalPointer::<&str>::reference(source_ptr);

    let text = node.utf8_text(source.as_bytes()).unwrap_or("");
    Ok(*RObject::from(text))
}

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_kind(node_ptr: SEXP) -> anyhow::Result<SEXP> {
    let node: Node<'static> = *(RAW(node_ptr) as *mut Node<'static>);

    Ok(*RObject::from(node.kind()))
}
