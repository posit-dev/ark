use oak_db::File;
use oak_db::OakDatabase;

/// Ark's salsa database view.
///
/// Extends oak_db's `Db` with queries we keep on the ark side. Right now that's
/// just the legacy tree-sitter tree. Defining it here keeps the `tree-sitter`
/// dependency out of `oak_db`, which is meant to stay a small shared crate. The
/// query still memoizes into the same `OakDatabase` storage, keyed on the `File`.
///
/// This mirrors how rust-analyzer layers database traits across crates: the
/// base crate owns `SourceDatabase`, downstream crates add their own traits on
/// top.
#[salsa::db]
pub(crate) trait ArkDb: oak_db::Db {}

#[salsa::db]
impl ArkDb for OakDatabase {}

/// Extension trait that adds ark-side query methods to `oak_db::File`.
pub(crate) trait FileArkExt {
    fn tree_sitter(self, db: &dyn ArkDb) -> &tree_sitter::Tree;
}

impl FileArkExt for File {
    fn tree_sitter(self, db: &dyn ArkDb) -> &tree_sitter::Tree {
        &tree_sitter_query(db, self).0
    }
}

/// Salsa wrapper around a tree-sitter `Tree`.
///
/// Salsa stores the memoized value and may overwrite it in place across
/// revisions, so the value has to implement `salsa::Update`. `Tree` doesn't,
/// and we can't implement a foreign trait on a foreign type, so we wrap it.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct TreeSitterTree(tree_sitter::Tree);

// SAFETY: `Tree` is fully owned, with no 'db references, so overwriting the old
// value in place is sound. We always report "changed" because tree-sitter trees
// aren't comparable. That pairs with `no_eq`, which already tells salsa never
// to backdate this query.
unsafe impl salsa::Update for TreeSitterTree {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        unsafe { *old_pointer = new_value };
        true
    }
}

/// Parse a file with tree-sitter.
///
/// `no_eq` because `tree_sitter::Tree` isn't `PartialEq`, so salsa can't
/// backdate it (same as `oak_db`'s `semantic_index`). `lru = 128` mirrors
/// `oak_db::File::parse`.
#[salsa::tracked(returns(ref), no_eq, lru = 128)]
fn tree_sitter_query(db: &dyn ArkDb, file: File) -> TreeSitterTree {
    TreeSitterTree(parse_tree_sitter(file.contents(db)))
}

/// Parse R source with tree-sitter.
///
/// The one place we build a tree-sitter parser. `tree_sitter_query()` runs it
/// over editor files; `statement_range` runs it over standalone snippets that
/// have no `oak_db::File`.
pub(crate) fn parse_tree_sitter(text: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();

    // Unwrap Safety: `tree-sitter-r` is a valid grammar; `set_language` only
    // fails on an ABI version mismatch, which is a build-time invariant.
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .unwrap();

    // Unwrap Safety: parsing without a timeout or cancellation flag never
    // returns `None`.
    parser.parse(text, None).unwrap()
}
