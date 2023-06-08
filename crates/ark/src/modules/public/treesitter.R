
#
# treesitter.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.treesitter.node.text <- function(node, source) {
    .ps.Call("ps_treesitter_node_text", node, source)
}

.ps.treesitter.node.kind <- function(node) {
    .ps.Call("ps_treesitter_node_kind", node)
}
