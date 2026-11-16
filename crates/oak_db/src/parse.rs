use aether_syntax::RRoot;
use aether_syntax::RSyntaxNode;
use biome_parser::diagnostic::ParseDiagnostic;
use biome_rowan::AstNode;
use biome_rowan::SendNode;

/// Thread-safe handle to a parsed R syntax tree.
///
/// Wraps biome_rowan's `SendNode` (a shareable language-agnostic root handle).
#[derive(Clone, Debug)]
pub(crate) struct OakParse {
    root: SendNode,
    diagnostics: Vec<ParseDiagnostic>,
}

impl OakParse {
    pub(crate) fn new(parse: aether_parser::RParse) -> Self {
        let root = parse
            .syntax()
            .as_send()
            .expect("`RParse::syntax()` returns the root node");

        Self {
            root,
            diagnostics: parse.into_diagnostics(),
        }
    }

    pub(crate) fn syntax(&self) -> RSyntaxNode {
        self.root
            .clone()
            .into_node()
            .expect("Aether's parser produces R-language trees")
    }

    pub(crate) fn tree(&self) -> RRoot {
        RRoot::unwrap_cast(self.syntax())
    }

    pub(crate) fn diagnostics(&self) -> &[ParseDiagnostic] {
        &self.diagnostics
    }
}

/// Equality is structural on just the tree (delegated to `GreenNode`'s
/// recursive compare) which is what lets Salsa backdate equal re-parses and
/// skip downstream queries.
///
/// The `ParseDiagnostic`s aren't `PartialEq` so they aren't included in the
/// equality check. But that's fine! We don't want them anyways. Comparing only
/// the trees is enough, since it means the source text is equivalent, implying
/// they have the same set of diagnostics.
impl PartialEq for OakParse {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root
    }
}

impl Eq for OakParse {}
