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
use crate::lsp::db::FileArkExt;

/// Editor-managed buffer state, paired with its `oak_db::File`.
///
/// `ArkFile` and `OakDatabase` are sibling fields on `WorldState`, so an
/// `ArkFile` cannot hold a reference to the database. That's why the methods
/// below take `db` as an argument instead of storing a reference, which is the
/// Salsa convention anyway.
#[derive(Debug)]
pub(crate) struct ArkFile {
    pub(crate) file: File,
    pub(crate) version: Option<i32>,
    pub(crate) config: DocumentConfig,
    pub(crate) url: Url,
    pub(crate) encoding: PositionEncoding,
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
        position: lsp_types::Position,
    ) -> anyhow::Result<tree_sitter::Point> {
        let line_col =
            from_proto::line_col_from_position(position, self.line_index(db), self.encoding);
        Ok(tree_sitter::Point::new(
            line_col.line as usize,
            line_col.col as usize,
        ))
    }

    pub(crate) fn lsp_position_from_tree_sitter_point(
        &self,
        db: &dyn ArkDb,
        point: tree_sitter::Point,
    ) -> anyhow::Result<lsp_types::Position> {
        let line_col = biome_line_index::LineCol {
            line: point.row as u32,
            col: point.column as u32,
        };
        to_proto::position_from_line_col(line_col, self.line_index(db), self.encoding)
    }

    pub(crate) fn lsp_range_from_tree_sitter_range(
        &self,
        db: &dyn ArkDb,
        range: tree_sitter::Range,
    ) -> anyhow::Result<lsp_types::Range> {
        let start = self.lsp_position_from_tree_sitter_point(db, range.start_point)?;
        let end = self.lsp_position_from_tree_sitter_point(db, range.end_point)?;
        Ok(lsp_types::Range::new(start, end))
    }

    pub(crate) fn tree_sitter_range_from_lsp_range(
        &self,
        db: &dyn ArkDb,
        range: lsp_types::Range,
    ) -> anyhow::Result<tree_sitter::Range> {
        let start_point = self.tree_sitter_point_from_lsp_position(db, range.start)?;
        let end_point = self.tree_sitter_point_from_lsp_position(db, range.end)?;

        let text_range = from_proto::text_range(range, self.line_index(db), self.encoding)?;

        Ok(tree_sitter::Range {
            start_byte: text_range.start().into(),
            end_byte: text_range.end().into(),
            start_point,
            end_point,
        })
    }
}

#[cfg(test)]
pub(crate) fn test_ark_file(code: &str) -> (oak_db::OakDatabase, ArkFile) {
    use aether_path::FilePath;

    let db = oak_db::OakDatabase::new();
    let url = Url::parse("file:///test.R").unwrap();
    let key = FilePath::from_url(&url);
    let file = ArkFile {
        file: File::new(&db, key, code.to_string(), None),
        version: None,
        config: DocumentConfig::default(),
        url,
        encoding: PositionEncoding::Wide(biome_line_index::WideEncoding::Utf16),
    };
    (db, file)
}
