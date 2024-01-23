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
use ropey::Rope;
use tree_sitter::Node;

use crate::lsp::traits::rope::RopeExt;

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_text(
    node_ptr: SEXP,
    source_ptr: SEXP,
) -> anyhow::Result<SEXP> {
    let node: Node<'static> = *(RAW(node_ptr) as *mut Node<'static>);
    let source = ExternalPointer::<Rope>::reference(source_ptr);

    let text = source
        .node_slice(&node)
        .map(|slice| slice.to_string())
        .unwrap_or(String::from(""));

    Ok(*RObject::from(text))
}

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_kind(node_ptr: SEXP) -> anyhow::Result<SEXP> {
    let node: Node<'static> = *(RAW(node_ptr) as *mut Node<'static>);

    Ok(*RObject::from(node.kind()))
}
