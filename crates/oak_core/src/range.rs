use biome_rowan::AstNode;
use biome_rowan::AstPtr;
use biome_rowan::SyntaxNode;
use biome_rowan::TextRange;

pub trait Ranged {
    fn range(&self) -> TextRange;
}

/// An [`AstPtr`] that also remembers the trimmed text range of the node it
/// captured.
///
/// A plain `AstPtr` stores only the with-trivia range and can't recover the
/// trimmed range without re-resolving against the tree. We snapshot the trimmed
/// range at capture, so a consumer that holds the pointer but not the tree gets
/// the exact token span for free.
pub struct RangedAstPtr<N: AstNode> {
    ptr: AstPtr<N>,
    range: TextRange,
}

impl<N: AstNode> RangedAstPtr<N> {
    pub fn new(node: &N) -> Self {
        Self {
            range: node.syntax().text_trimmed_range(),
            ptr: AstPtr::new(node),
        }
    }

    /// The trimmed range which excludes surrounding whitespace and comments,
    /// snapshotted at creation.
    pub fn text_trimmed_range(&self) -> TextRange {
        self.range
    }

    /// The range including leading and trailing trivia, read back from the
    /// pointer, which already stores it.
    pub fn text_range_with_trivia(&self) -> TextRange {
        self.ptr.syntax_node_ptr().text_range()
    }

    /// Resolve back to the node against the tree that produced it.
    pub fn to_node(&self, root: &SyntaxNode<N::Language>) -> N {
        self.ptr.to_node(root)
    }

    /// The underlying pointer, for consumers that only need the handle.
    pub fn as_ptr(&self) -> &AstPtr<N> {
        &self.ptr
    }
}

// Hand-written like `AstPtr`'s own impls, so the bounds don't demand `N: Clone`
// or `N: Debug`.
impl<N: AstNode> Clone for RangedAstPtr<N> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr.clone(),
            range: self.range,
        }
    }
}

impl<N: AstNode> std::fmt::Debug for RangedAstPtr<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RangedAstPtr")
            .field("ptr", &self.ptr)
            .field("range", &self.range)
            .finish()
    }
}
