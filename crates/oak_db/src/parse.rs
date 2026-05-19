use aether_syntax::RRoot;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::SendNode;

/// Thread-safe handle to a parsed R syntax tree.
///
/// Wraps biome_rowan's `SendNode` (a shareable language-agnostic root handle).
/// Equality is structural (delegated to `GreenNode`'s recursive compare) which
/// is what lets Salsa backdate equal re-parses and skip downstream queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OakParse(SendNode);

impl OakParse {
    pub(crate) fn new(parse: aether_parser::Parse) -> Self {
        Self(
            parse
                .syntax()
                .as_send()
                .expect("`Parse::syntax()` returns the root node"),
        )
    }

    pub(crate) fn syntax(&self) -> RSyntaxNode {
        self.0
            .clone()
            .into_node()
            .expect("Aether's parser produces R-language trees")
    }

    pub(crate) fn tree(&self) -> RRoot {
        RRoot::unwrap_cast(self.syntax())
    }
}
