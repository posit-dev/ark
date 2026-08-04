use aether_path::FilePath;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::ScopeId;
use rustc_hash::FxHashMap;

use crate::Db;
use crate::File;

/// A `source()` call (or more generally a `Source` effect), from the file it's
/// written in to the file it names.
///
/// Both ends are carried so that a site is enough on its own to anchor a
/// diagnostic in the sourcing file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSite {
    file: File,
    target: Option<File>,
    path: String,
    offset: TextSize,
    scope: ScopeId,
}

impl SourceSite {
    /// The file the `source()` call is written in.
    pub fn file(&self) -> File {
        self.file
    }

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
    /// The `source()` calls written in this file, each naming an immediate
    /// target. A file sourced by a file this one sources gets no entry.
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
                    file: self,
                    target,
                    path: path.clone(),
                    offset: call.offset(),
                    scope: call.scope(),
                })
            })
            .collect()
    }

    /// The workspace files `self` sources, sorted by path and deduplicated.
    ///
    /// Omits source-call offsets so text edits that preserve targets do not
    /// invalidate [`sourcing_files_by_target`].
    #[salsa::tracked(returns(ref))]
    pub(crate) fn source_targets(self, db: &dyn Db) -> Vec<File> {
        let mut targets: Vec<File> = self
            .source_sites(db)
            .iter()
            .filter_map(|site| site.target())
            .collect();

        targets.sort_by_cached_key(|target| target.path(db).to_string());
        targets.dedup();
        targets
    }

    /// The workspace files that source `self`.
    ///
    /// Sorted by path and deduped. This is a firewall query that deliberately
    /// does not carry offsets. Use [`File::source_sites`] to get the call
    /// positions.
    ///
    /// No `cycle_result` since `semantic_index` construction does not read this query.
    #[salsa::tracked(returns(ref))]
    pub fn sourced_by(self, db: &dyn Db) -> Vec<File> {
        sourcing_files_by_target(db)
            .get(&self)
            .cloned()
            .unwrap_or_default()
    }
}

/// For each file in the workspace, the files that source it.
///
/// Sorted by path and deduped. `workspace_files()`' iteration order isn't
/// a stable contract, and an unstable order here would prevent
/// [`File::sourced_by`] from backdating.
#[salsa::tracked(returns(ref))]
fn sourcing_files_by_target(db: &dyn Db) -> FxHashMap<File, Vec<File>> {
    let mut by_target: FxHashMap<File, Vec<File>> = FxHashMap::default();

    for &file in crate::workspace_files(db) {
        for &target in file.source_targets(db) {
            by_target.entry(target).or_default().push(file);
        }
    }

    // Deduplicate because `workspace_files()` can include a file through its root
    // and package.
    for files in by_target.values_mut() {
        files.sort_by_cached_key(|file| file.path(db).to_string());
        files.dedup();
    }

    by_target
}
