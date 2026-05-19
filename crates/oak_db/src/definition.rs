use biome_rowan::TextRange;
use oak_semantic::semantic_index::ScopeId;

use crate::File;
use crate::Name;

/// A name binding identified by `(file, scope, name)`.
///
/// `Definition` is a salsa-tracked entity. Identity is determined by the
/// untracked fields, so the same `(file, scope, name)` tuple maps to the same
/// salsa id across revisions, even when the source position of the binding
/// shifts. This mirrors ty's `Definition<'db>` shape (see
/// `crates/ty_python_semantic/src/semantic_index/definition.rs`).
///
/// The `range` is a `#[tracked]` field, so it updates per revision and
/// re-execution of any consumer that reads it picks up the new range. Consumers
/// that only read identity (file / scope / name) see no change across body
/// edits.
///
/// `no_eq` on `range` because `TextRange` shifts on any edit moving the
/// binding's line, so backdating would never fire. The point isn't backdating,
/// it's per-field invalidation.
#[salsa::tracked(debug)]
pub struct Definition<'db> {
    /// The file containing the binding.
    pub file: File,
    /// The scope within `file` where the binding is introduced.
    pub scope: ScopeId,
    /// The interned name being bound. Stable across revisions for the
    /// same identifier text.
    pub name: Name<'db>,
    /// Source range of the bound identifier. Shifts on edits.
    #[tracked]
    #[no_eq]
    pub range: TextRange,
}
