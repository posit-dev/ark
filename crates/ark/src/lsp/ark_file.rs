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
/// This is a temporary structure during the transition to pure Oak handlers.
///
/// The methods take `db` as a parameter rather than holding it. `ArkFile` lives
/// in `WorldState`, and the db is a sibling field there, so a stored borrow of
/// it would be self-referential, which safe Rust forbids. Passing `db` per call
/// is the salsa idiom anyway (`file.parse(db)`).
#[derive(Clone, Debug)]
pub(crate) struct ArkFile {
    pub(crate) file: File,
    pub(crate) version: Option<i32>,
    pub(crate) config: DocumentConfig,
    // The editor's verbatim URL. We store it rather than recompute it from
    // `file`'s path so the bytes the frontend sent round-trip exactly. It lives
    // on `ArkFile` so it travels with owned values for callers that can't
    // easily access `WorldState::open_files`: the diagnostics task on a worker
    // thread (`RefreshDiagnosticsTask`) and `code_action/roxygen.rs`, which
    // builds a `WorkspaceEdit` keyed by URL.
    //
    // TODO: this is a stopgap that goes away with `ArkFile`. Once handlers are
    // pure Oak, they return `File`-keyed results (diagnostics, the edit targets
    // in a `WorkspaceEdit`) and the wire URL gets attached at the transport
    // boundary from a map of open editor URLs owned by the LSP layer. In that
    // design the verbatim URL never travels through the analysis layer.
    pub(crate) wire_url: Url,
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
        self.file.source_text(db).as_str()
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
        lsp_position_from_tree_sitter_point(point, self.line_index(db), self.encoding)
    }

    pub(crate) fn lsp_range_from_tree_sitter_range(
        &self,
        db: &dyn ArkDb,
        range: tree_sitter::Range,
    ) -> anyhow::Result<lsp_types::Range> {
        lsp_range_from_tree_sitter_range(range, self.line_index(db), self.encoding)
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

/// Free functions over `LineIndex` + `PositionEncoding`, so anything holding
/// those two (an `ArkFile` plus its `db`, or a `DocumentContext`) can convert
/// without each carrying its own copy of the logic.
pub(crate) fn lsp_position_from_tree_sitter_point(
    point: tree_sitter::Point,
    line_index: &biome_line_index::LineIndex,
    encoding: PositionEncoding,
) -> anyhow::Result<lsp_types::Position> {
    let line_col = biome_line_index::LineCol {
        line: point.row as u32,
        col: point.column as u32,
    };
    to_proto::position_from_line_col(line_col, line_index, encoding)
}

pub(crate) fn lsp_range_from_tree_sitter_range(
    range: tree_sitter::Range,
    line_index: &biome_line_index::LineIndex,
    encoding: PositionEncoding,
) -> anyhow::Result<lsp_types::Range> {
    let start = lsp_position_from_tree_sitter_point(range.start_point, line_index, encoding)?;
    let end = lsp_position_from_tree_sitter_point(range.end_point, line_index, encoding)?;
    Ok(lsp_types::Range::new(start, end))
}

#[cfg(test)]
pub(crate) fn test_ark_file(code: &str) -> (oak_db::OakDatabase, ArkFile) {
    use aether_path::FilePath;

    let db = oak_db::OakDatabase::new();
    let url = Url::parse("file:///test.R").unwrap();
    let key = FilePath::from_url(&url);
    let file = ArkFile {
        file: File::new(
            &db,
            key,
            oak_db::FileRevision::zero(),
            Some(code.to_string()),
            None,
        ),
        version: None,
        config: DocumentConfig::default(),
        wire_url: url,
        encoding: PositionEncoding::Wide(biome_line_index::WideEncoding::Utf16),
    };
    (db, file)
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use super::*;

    #[test]
    fn test_tree_sitter_point_from_lsp_position_wide_encoding() {
        // The emoji is 4 UTF-8 bytes and 2 UTF-16 bytes
        // `test_ark_file` defaults to UTF-16, the encoding under test here.
        let (db, ark_file) = test_ark_file("😃a");

        let point = ark_file
            .tree_sitter_point_from_lsp_position(&db, lsp_types::Position::new(0, 2))
            .unwrap();
        assert_eq!(point, Point::new(0, 4));

        let point = ark_file
            .tree_sitter_point_from_lsp_position(&db, lsp_types::Position::new(0, 3))
            .unwrap();
        assert_eq!(point, Point::new(0, 5));
    }

    #[test]
    fn test_lsp_position_from_tree_sitter_point_wide_encoding() {
        let (db, ark_file) = test_ark_file("😃a");

        let position = ark_file
            .lsp_position_from_tree_sitter_point(&db, Point::new(0, 4))
            .unwrap();
        assert_eq!(position, lsp_types::Position::new(0, 2));

        let position = ark_file
            .lsp_position_from_tree_sitter_point(&db, Point::new(0, 5))
            .unwrap();
        assert_eq!(position, lsp_types::Position::new(0, 3));
    }

    #[test]
    fn test_utf8_position_roundtrip_multibyte() {
        // `é` is 2 bytes
        let (db, mut ark_file) = test_ark_file("é\n");
        ark_file.encoding = PositionEncoding::Utf8;

        let lsp_position = lsp_types::Position::new(0, 2);
        let point = ark_file
            .tree_sitter_point_from_lsp_position(&db, lsp_position)
            .unwrap();
        assert_eq!(point, Point::new(0, 2));

        let roundtrip_position = ark_file
            .lsp_position_from_tree_sitter_point(&db, point)
            .unwrap();
        assert_eq!(roundtrip_position, lsp_position);
    }
}
