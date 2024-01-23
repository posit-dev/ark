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
    ffi_node: SEXP,
    ffi_contents: SEXP,
) -> anyhow::Result<SEXP> {
    let node: Node<'static> = *(RAW(ffi_node) as *mut Node<'static>);
    let contents = ExternalPointer::<Rope>::reference(ffi_contents);

    let text = contents
        .node_slice(&node)
        .map(|slice| slice.to_string())
        .unwrap_or(String::from(""));

    Ok(*RObject::from(text))
}

#[harp::register]
pub unsafe extern "C" fn ps_treesitter_node_kind(ffi_node: SEXP) -> anyhow::Result<SEXP> {
    let node: Node<'static> = *(RAW(ffi_node) as *mut Node<'static>);

    Ok(*RObject::from(node.kind()))
}
