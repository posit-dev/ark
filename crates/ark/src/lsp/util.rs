//
// util.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::os::raw::c_char;

use anyhow::anyhow;
use harp::object::RObject;
use libr::R_NilValue;
use libr::Rf_mkString;
use libr::SEXP;
use stdext::unwrap;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::TextEdit;

/// Apply text edits to a string
///
/// The edits are applied in order as documented in the LSP protocol.
/// A good strategy is to sort them from bottom to top.
pub fn apply_text_edits(edits: Vec<TextEdit>, text: &mut String) -> anyhow::Result<()> {
    for edit in edits {
        let start = unwrap!(position_as_offset(text, edit.range.start), None => {
            return Err(anyhow!("Can't apply edit {edit:?} because start is OOB"));
        });
        let end = unwrap!(position_as_offset(text, edit.range.end), None => {
            return Err(anyhow!("Can't apply edit {edit:?} because end is OOB"));
        });

        text.replace_range(start..end, &edit.new_text)
    }
    Ok(())
}

fn position_as_offset(text: &str, pos: Position) -> Option<usize> {
    line_offset(text, pos.line as usize).map(|offset| offset + pos.character as usize)
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

/// Shows a message in the Positron frontend
#[harp::register]
pub unsafe extern "C" fn ps_log_error(message: SEXP) -> anyhow::Result<SEXP> {
    let message = RObject::view(message).to::<String>();
    if let Ok(message) = message {
        log::error!("{}", message);
    }

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_object_id(object: SEXP) -> anyhow::Result<SEXP> {
    let value = format!("{:p}", object);
    return Ok(Rf_mkString(value.as_ptr() as *const c_char));
}

#[cfg(test)]
mod tests {
    use harp::assert_match;
    use tower_lsp::lsp_types::Position;
    use tower_lsp::lsp_types::Range;
    use tower_lsp::lsp_types::TextEdit;

    use crate::lsp::util::apply_text_edits;
    use crate::lsp::util::line_offset;

    #[test]
    fn test_apply_edit() {
        let edits = vec![
            TextEdit {
                range: Range {
                    start: Position {
                        line: 1,
                        character: 0,
                    },
                    end: Position {
                        line: 1,
                        character: 3,
                    },
                },
                new_text: String::from("qux"),
            },
            TextEdit {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 3,
                    },
                    end: Position {
                        line: 0,
                        character: 4,
                    },
                },
                new_text: String::from(""),
            },
        ];

        let mut text = String::from("foo bar\nbaz");
        assert!(apply_text_edits(edits, &mut text).is_ok());
        assert_eq!(text, String::from("foobar\nqux"));

        let edits = vec![TextEdit {
            range: Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 3,
                },
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
