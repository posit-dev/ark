#
# treesitter.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.treesitter.node.text <- function(node, contents) {
    .ps.Call("ps_treesitter_node_text", node, contents)
}

#' @export
.ps.treesitter.node.kind <- function(node) {
    .ps.Call("ps_treesitter_node_kind", node)
}
