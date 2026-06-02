//
// ark_file.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use oak_db::File;
use tower_lsp::lsp_types;
use url::Url;

use crate::lsp::config::DocumentConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::db::FileExt;

/// Editor-managed buffer state, paired with its `oak_db::File`.
///
/// This is the LSP's replacement for the analysis-carrying `Document`. The
/// protocol fields (`version`, `config`, `url`) are plain data. Everything
/// derived from the buffer text (the tree-sitter tree, the line index, the
/// contents) is reached through salsa queries on `file`, so it's computed once
/// and never stored twice.
///
/// For now `ArkFile` is built per request, from the still-stored `Document`
/// plus the `File` handle. Once `Document` is gone it becomes the value stored
/// in `WorldState::documents`.
///
/// The methods take `db` as a parameter rather than holding it. `ArkFile` lives
/// in `WorldState`, and the db is a sibling field there, so a stored borrow of
/// it would be self-referential, which safe Rust forbids. Passing `db` per call
/// is the salsa idiom anyway (`file.parse(db)`).
pub(crate) struct ArkFile {
    pub(crate) file: File,
    pub(crate) version: Option<i32>,
    pub(crate) config: DocumentConfig,
    #[allow(dead_code)]
    pub(crate) url: Url,
}

impl ArkFile {
    pub(crate) fn tree_sitter<'db>(&self, db: &'db dyn ArkDb) -> &'db tree_sitter::Tree {
        self.file.tree_sitter(db)
    }

    pub(crate) fn line_index<'db>(&self, db: &'db dyn ArkDb) -> &'db biome_line_index::LineIndex {
        self.file.line_index(db)
    }

    pub(crate) fn contents<'db>(&self, db: &'db dyn ArkDb) -> &'db str {
        self.file.contents(db).as_str()
    }

    pub(crate) fn get_line<'db>(&self, db: &'db dyn ArkDb, line: usize) -> Option<&'db str> {
        let line_index = self.line_index(db);
        let contents = self.contents(db);

        let Some(line_start) = line_index.newlines.get(line) else {
            // Forcing a full capture so we can learn the situations in which this occurs
            log::error!(
                "Requesting line {line} but only {n} lines exist.\n\nContents:\n{contents}\n\nBacktrace:\n{trace}",
                n = line_index.len(),
                line = line + 1,
                trace = std::backtrace::Backtrace::force_capture(),
            );
            return None;
        };

        let line_end = line_index
            .newlines
            .get(line + 1)
            .copied()
            // if `line` is last, extract text until end of buffer
            .unwrap_or_else(|| (contents.len() as u32).into());

        let line_start_byte: usize = line_start.to_owned().into();
        let line_end_byte: usize = line_end.into();

        contents.get(line_start_byte..line_end_byte)
    }

    pub(crate) fn tree_sitter_point_from_lsp_position(
        &self,
        db: &dyn ArkDb,
        encoding: PositionEncoding,
        position: lsp_types::Position,
    ) -> anyhow::Result<tree_sitter::Point> {
        let line_col = from_proto::line_col_from_position(position, self.line_index(db), encoding);
        Ok(tree_sitter::Point::new(
            line_col.line as usize,
            line_col.col as usize,
        ))
    }

    pub(crate) fn lsp_position_from_tree_sitter_point(
        &self,
        db: &dyn ArkDb,
        encoding: PositionEncoding,
        point: tree_sitter::Point,
    ) -> anyhow::Result<lsp_types::Position> {
        let line_col = biome_line_index::LineCol {
            line: point.row as u32,
            col: point.column as u32,
        };
        to_proto::position_from_line_col(line_col, self.line_index(db), encoding)
    }

    pub(crate) fn lsp_range_from_tree_sitter_range(
        &self,
        db: &dyn ArkDb,
        encoding: PositionEncoding,
        range: tree_sitter::Range,
    ) -> anyhow::Result<lsp_types::Range> {
        let start = self.lsp_position_from_tree_sitter_point(db, encoding, range.start_point)?;
        let end = self.lsp_position_from_tree_sitter_point(db, encoding, range.end_point)?;
        Ok(lsp_types::Range::new(start, end))
    }

    pub(crate) fn tree_sitter_range_from_lsp_range(
        &self,
        db: &dyn ArkDb,
        encoding: PositionEncoding,
        range: lsp_types::Range,
    ) -> anyhow::Result<tree_sitter::Range> {
        let start_point = self.tree_sitter_point_from_lsp_position(db, encoding, range.start)?;
        let end_point = self.tree_sitter_point_from_lsp_position(db, encoding, range.end)?;

        let text_range = from_proto::text_range(range, self.line_index(db), encoding)?;

        Ok(tree_sitter::Range {
            start_byte: text_range.start().into(),
            end_byte: text_range.end().into(),
            start_point,
            end_point,
        })
    }
}

#[cfg(test)]
pub(crate) fn ark_file_for_test(code: &str) -> (oak_db::OakDatabase, ArkFile) {
    use aether_path::FilePath;

    let db = oak_db::OakDatabase::new();
    let url = Url::parse("file:///test.R").unwrap();
    let key = FilePath::from_url(&url);
    let file = File::new(&db, key, code.to_string(), None);
    let ark_file = ArkFile {
        file,
        version: None,
        config: DocumentConfig::default(),
        url,
    };
    (db, ark_file)
}
