use oak_db::Db;
use oak_db::File;
use tower_lsp_server::ls_types as lsp_types;

use crate::lsp::config::DocumentConfig;
#[cfg(test)]
use crate::lsp::db::ArkDb;
#[cfg(test)]
use crate::lsp::db::FileArkExt;

/// An editor-managed buffer: the salsa `File` plus the editor and transport
/// state that should not cross into the analysis layer.
#[derive(Clone, Debug)]
pub(crate) struct OpenFile {
    file: File,
    version: Option<i32>,
    config: DocumentConfig,
    wire_uri: lsp_types::Uri,
}

impl OpenFile {
    pub(crate) fn new(file: File, version: Option<i32>, wire_uri: lsp_types::Uri) -> Self {
        Self {
            file,
            version,
            config: DocumentConfig::default(),
            wire_uri,
        }
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }

    /// `OpenFile` forwards the salsa read queries so a caller holding a buffer
    /// can ask for its text or line index directly instead of reaching through
    /// `file()`.
    pub(crate) fn source_text<'db>(&self, db: &'db dyn Db) -> &'db String {
        self.file.source_text(db)
    }
    pub(crate) fn line_index<'db>(&self, db: &'db dyn Db) -> &'db biome_line_index::LineIndex {
        self.file.line_index(db)
    }

    /// Lib handlers reach the tree-sitter tree through the raw `File`. Only test
    /// helpers hold an `OpenFile` and want the tree directly, so this forwarder
    /// is test-only.
    #[cfg(test)]
    pub(crate) fn tree_sitter<'db>(&self, db: &'db dyn ArkDb) -> &'db tree_sitter::Tree {
        self.file.tree_sitter(db)
    }

    pub(crate) fn version(&self) -> Option<i32> {
        self.version
    }
    pub(crate) fn config(&self) -> &DocumentConfig {
        &self.config
    }

    /// The verbatim `Uri` the editor sent us, preserved so wire output
    /// replays it exactly. See [`crate::lsp::state::WorldState::wire_uri`].
    pub(crate) fn wire_uri(&self) -> &lsp_types::Uri {
        &self.wire_uri
    }

    pub(crate) fn set_version(&mut self, version: Option<i32>) {
        self.version = version;
    }
    pub(crate) fn config_mut(&mut self) -> &mut DocumentConfig {
        &mut self.config
    }
}
