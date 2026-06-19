use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use oak_db::Db;
use oak_db::File;
use tower_lsp::lsp_types;
use url::Url;

use crate::lsp::config::DocumentConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::db::FileArkExt;

/// An editor-managed buffer: the salsa `File` plus the editor and transport
/// state that should not cross into the analysis layer.
#[derive(Clone, Debug)]
pub(crate) struct OpenFile {
    file: File,
    version: Option<i32>,
    config: DocumentConfig,
    wire_url: Url,
}

impl OpenFile {
    pub(crate) fn new(file: File, version: Option<i32>, wire_url: Url) -> Self {
        Self {
            file,
            version,
            config: DocumentConfig::default(),
            wire_url,
        }
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }

    /// `OpenFile` forwards the salsa read queries so a caller holding a buffer
    /// can ask for its text, line index, or tree directly instead of reaching
    /// through `file()`.
    pub(crate) fn source_text<'db>(&self, db: &'db dyn Db) -> &'db String {
        self.file.source_text(db)
    }
    pub(crate) fn line_index<'db>(&self, db: &'db dyn Db) -> &'db biome_line_index::LineIndex {
        self.file.line_index(db)
    }
    pub(crate) fn tree_sitter<'db>(&self, db: &'db dyn ArkDb) -> &'db tree_sitter::Tree {
        self.file.tree_sitter(db)
    }

    pub(crate) fn version(&self) -> Option<i32> {
        self.version
    }
    pub(crate) fn config(&self) -> &DocumentConfig {
        &self.config
    }

    /// The verbatim editor URL, preserved so wire output echoes the URI the
    /// editor sent us. See [`crate::lsp::state::WorldState::wire_url`].
    pub(crate) fn wire_url(&self) -> &Url {
        &self.wire_url
    }

    pub(crate) fn set_version(&mut self, version: Option<i32>) {
        self.version = version;
    }
    pub(crate) fn config_mut(&mut self) -> &mut DocumentConfig {
        &mut self.config
    }
}

/// Free functions over `LineIndex` + `PositionEncoding`, so anything holding
/// those two (a `File` plus its `db`, or a `DocumentContext`) can convert
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

pub(crate) fn tree_sitter_point_from_lsp_position(
    position: lsp_types::Position,
    line_index: &biome_line_index::LineIndex,
    encoding: PositionEncoding,
) -> anyhow::Result<tree_sitter::Point> {
    let line_col = from_proto::line_col_from_position(position, line_index, encoding);
    Ok(tree_sitter::Point::new(
        line_col.line as usize,
        line_col.col as usize,
    ))
}

pub(crate) fn tree_sitter_range_from_lsp_range(
    range: lsp_types::Range,
    line_index: &biome_line_index::LineIndex,
    encoding: PositionEncoding,
) -> anyhow::Result<tree_sitter::Range> {
    let start_point = tree_sitter_point_from_lsp_position(range.start, line_index, encoding)?;
    let end_point = tree_sitter_point_from_lsp_position(range.end, line_index, encoding)?;

    let text_range = from_proto::text_range(range, line_index, encoding)?;

    Ok(tree_sitter::Range {
        start_byte: text_range.start().into(),
        end_byte: text_range.end().into(),
        start_point,
        end_point,
    })
}

pub(crate) fn get_line<'a>(
    contents: &'a str,
    line_index: &biome_line_index::LineIndex,
    line: usize,
) -> Option<&'a str> {
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

#[cfg(test)]
pub(crate) fn test_open_file(code: &str) -> (oak_db::OakDatabase, OpenFile) {
    use aether_path::FilePath;

    let db = oak_db::OakDatabase::new();
    let url = Url::parse("file:///test.R").unwrap();
    let key = FilePath::from_url(&url);
    let inner = File::new(
        &db,
        key,
        oak_db::FileRevision::zero(),
        Some(code.to_string()),
        None,
    );
    let file = OpenFile::new(inner, None, url);
    (db, file)
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use super::*;

    const ENCODING: PositionEncoding =
        PositionEncoding::Wide(biome_line_index::WideEncoding::Utf16);

    #[test]
    fn test_tree_sitter_point_from_lsp_position_wide_encoding() {
        // The emoji is 4 UTF-8 bytes and 2 UTF-16 bytes
        let (db, file) = test_open_file("😃a");
        let line_index = file.line_index(&db);

        let point = tree_sitter_point_from_lsp_position(
            lsp_types::Position::new(0, 2),
            line_index,
            ENCODING,
        )
        .unwrap();
        assert_eq!(point, Point::new(0, 4));

        let point = tree_sitter_point_from_lsp_position(
            lsp_types::Position::new(0, 3),
            line_index,
            ENCODING,
        )
        .unwrap();
        assert_eq!(point, Point::new(0, 5));
    }

    #[test]
    fn test_lsp_position_from_tree_sitter_point_wide_encoding() {
        let (db, file) = test_open_file("😃a");
        let line_index = file.line_index(&db);

        let position =
            lsp_position_from_tree_sitter_point(Point::new(0, 4), line_index, ENCODING).unwrap();
        assert_eq!(position, lsp_types::Position::new(0, 2));

        let position =
            lsp_position_from_tree_sitter_point(Point::new(0, 5), line_index, ENCODING).unwrap();
        assert_eq!(position, lsp_types::Position::new(0, 3));
    }

    #[test]
    fn test_utf8_position_roundtrip_multibyte() {
        // `é` is 2 bytes
        let (db, file) = test_open_file("é\n");
        let line_index = file.line_index(&db);
        let encoding = PositionEncoding::Utf8;

        let lsp_position = lsp_types::Position::new(0, 2);
        let point =
            tree_sitter_point_from_lsp_position(lsp_position, line_index, encoding).unwrap();
        assert_eq!(point, Point::new(0, 2));

        let roundtrip_position =
            lsp_position_from_tree_sitter_point(point, line_index, encoding).unwrap();
        assert_eq!(roundtrip_position, lsp_position);
    }
}
