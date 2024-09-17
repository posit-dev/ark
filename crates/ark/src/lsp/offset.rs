//
// offset.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

// UTF-8-based types for internal usage Currently uses TS types
// TODO: Consider using https://github.com/rust-analyzer/text-size/

use anyhow::anyhow;
use tower_lsp::lsp_types;
pub use tree_sitter::Point as ArkPoint;

use crate::lsp::encoding::convert_point_to_position;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArkRange {
    pub start: ArkPoint,
    pub end: ArkPoint,
}

impl From<harp::srcref::SrcRef> for ArkRange {
    fn from(value: harp::srcref::SrcRef) -> Self {
        ArkRange {
            start: ArkPoint {
                row: value.line.start,
                column: value.column.start,
            },
            end: ArkPoint {
                row: value.line.end,
                column: value.column.end,
            },
        }
    }
}

/// Like `TextEdit` from the lsp_types crate, but doen't expect positions to be
/// encoded in UTF-16.
#[derive(Clone, Debug)]
pub struct ArkTextEdit {
    pub range: ArkRange,
    pub new_text: String,
}

pub trait FromArkOffset<T>: Sized {
    fn from_ark_offset(text: &ropey::Rope, value: T) -> Self;
}

pub trait IntoLspOffset<T>: Sized {
    fn into_lsp_offset(self, text: &ropey::Rope) -> T;
}

impl<T, U> IntoLspOffset<U> for T
where
    U: FromArkOffset<T>,
{
    fn into_lsp_offset(self, text: &ropey::Rope) -> U {
        U::from_ark_offset(text, self)
    }
}

impl FromArkOffset<ArkPoint> for lsp_types::Position {
    fn from_ark_offset(text: &ropey::Rope, value: ArkPoint) -> lsp_types::Position {
        let point = tree_sitter::Point {
            row: value.row,
            column: value.column,
        };
        convert_point_to_position(text, point)
    }
}

impl FromArkOffset<ArkRange> for lsp_types::Range {
    fn from_ark_offset(text: &ropey::Rope, value: ArkRange) -> lsp_types::Range {
        lsp_types::Range {
            start: value.start.into_lsp_offset(text),
            end: value.end.into_lsp_offset(text),
        }
    }
}

impl FromArkOffset<ArkTextEdit> for lsp_types::TextEdit {
    fn from_ark_offset(text: &ropey::Rope, value: ArkTextEdit) -> lsp_types::TextEdit {
        lsp_types::TextEdit {
            range: value.range.into_lsp_offset(text),
            new_text: value.new_text,
        }
    }
}

impl FromArkOffset<Vec<ArkTextEdit>> for Vec<lsp_types::TextEdit> {
    fn from_ark_offset(text: &ropey::Rope, value: Vec<ArkTextEdit>) -> Vec<lsp_types::TextEdit> {
        value
            .into_iter()
            .map(|edit| edit.into_lsp_offset(text))
            .collect()
    }
}

/// Apply text edits to a string
///
/// The edits are applied in order as documented in the LSP protocol.
/// A good strategy is to sort them from bottom to top.
pub fn apply_text_edits(edits: Vec<ArkTextEdit>, text: &mut String) -> anyhow::Result<()> {
    for edit in edits {
        let Some(start) = point_as_offset(text, edit.range.start) else {
            return Err(anyhow!("Can't apply edit {edit:?} because start is OOB"));
        };
        let Some(end) = point_as_offset(text, edit.range.end) else {
            return Err(anyhow!("Can't apply edit {edit:?} because end is OOB"));
        };

        text.replace_range(start..end, &edit.new_text)
    }
    Ok(())
}

fn point_as_offset(text: &str, point: ArkPoint) -> Option<usize> {
    line_offset(text, point.row).map(|offset| offset + point.column)
}

fn line_offset(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }

    text.chars()
        .enumerate()
        .filter(|(_, c)| *c == '\n')
        .skip(line - 1)
        .next()
        .map(|res| res.0 + 1)
}

#[cfg(test)]
mod tests {
    use harp::assert_match;

    use crate::lsp::offset::apply_text_edits;
    use crate::lsp::offset::line_offset;
    use crate::lsp::offset::ArkPoint;
    use crate::lsp::offset::ArkRange;
    use crate::lsp::offset::ArkTextEdit;

    #[test]
    fn test_apply_edit() {
        let edits = vec![
            ArkTextEdit {
                range: ArkRange {
                    start: ArkPoint { row: 1, column: 0 },
                    end: ArkPoint { row: 1, column: 3 },
                },
                new_text: String::from("qux"),
            },
            ArkTextEdit {
                range: ArkRange {
                    start: ArkPoint { row: 0, column: 3 },
                    end: ArkPoint { row: 0, column: 4 },
                },
                new_text: String::from(""),
            },
        ];

        let mut text = String::from("foo bar\nbaz");
        assert!(apply_text_edits(edits, &mut text).is_ok());
        assert_eq!(text, String::from("foobar\nqux"));

        let edits = vec![ArkTextEdit {
            range: ArkRange {
                start: ArkPoint { row: 1, column: 0 },
                end: ArkPoint { row: 1, column: 3 },
            },
            new_text: String::from("qux"),
        }];

        let mut text = String::from("foo");
        assert_match!(apply_text_edits(edits, &mut text), Err(_));
    }

    #[test]
    fn test_line_pos() {
        assert_eq!(line_offset("", 0), Some(0));
        assert_eq!(line_offset("", 1), None);

        assert_eq!(line_offset("\n", 0), Some(0));
        assert_eq!(line_offset("\n", 1), Some(1));
        assert_eq!(line_offset("\n", 2), None);

        let text = "foo\nquux\nbaz";
        assert_eq!(line_offset(text, 0), Some(0));
        assert_eq!(line_offset(text, 1), Some(4));
        assert_eq!(line_offset(text, 2), Some(9));
        assert_eq!(line_offset(text, 3), None);
    }
}
