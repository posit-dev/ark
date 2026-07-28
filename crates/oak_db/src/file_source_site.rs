use aether_path::FilePath;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::ScopeId;

use crate::Db;
use crate::File;

/// A `source()` call (or more generally a `Source` effect) in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSite {
    target: Option<File>,
    path: String,
    offset: TextSize,
    scope: ScopeId,
}

impl SourceSite {
    /// The workspace file the sourced path resolved to. `None` when the path
    /// didn't resolve, or resolved outside the workspace.
    pub fn target(&self) -> Option<File> {
        self.target
    }

    /// The sourced path as written in the `source()` call.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn offset(&self) -> TextSize {
        self.offset
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
    }
}

#[salsa::tracked]
impl File {
    /// The `source()` calls in this file, each naming an immediate target.
    ///
    /// A call that didn't resolve is kept anyway, so a consumer can report the
    /// path the user wrote.
    ///
    /// No `cycle_result`: `semantic_index` construction does not call this query.
    #[salsa::tracked(returns(ref))]
    pub fn source_sites(self, db: &dyn Db) -> Vec<SourceSite> {
        self.semantic_index(db)
            .semantic_calls()
            .iter()
            .filter_map(|call| {
                let SemanticCallKind::Source { path, resolved } = call.kind() else {
                    return None;
                };
                let target = resolved
                    .as_ref()
                    .and_then(|url| db.file_by_path(&FilePath::from_url(url)));
                Some(SourceSite {
                    target,
                    path: path.clone(),
                    offset: call.offset(),
                    scope: call.scope(),
                })
            })
            .collect()
    }
}
