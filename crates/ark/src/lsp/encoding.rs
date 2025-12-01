//
// encoding.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use biome_line_index::LineIndex;
use tower_lsp::lsp_types;

/// `PositionEncodingKind` describes the encoding used for the `Position` `character`
/// column offset field. The `Position` `line` field is encoding agnostic, but the
/// `character` field specifies the number of characters offset from the beginning of
/// the line, and the "character" size is dependent on the encoding. The LSP specification
/// states:
///
/// - UTF8: Character offsets count UTF-8 code units (e.g. bytes).
/// - UTF16: Character offsets count UTF-16 code units (default).
/// - UTF32: Character offsets count UTF-32 code units (these are the same as Unicode
///   codepoints, so this `PositionEncodingKind` may also be used for an encoding-agnostic
///   representation of character offsets.)
///
/// The `vscode-languageclient` library that Positron uses on the frontend to create the
/// `Client` side of the LSP currently ONLY supports `UTF16`, and will error on anything
/// else. Their reasoning is that it is easier for the server (ark) to do the re-encoding,
/// since we are tracking the full document state. Track support for UTF-8 here:
/// https://github.com/microsoft/vscode-languageserver-node/issues/1224
///
/// The other interesting part of this is that `TextDocumentContentChangeEvent`s that
/// come through the `did_change()` event and the `TextDocumentItem` that comes through
/// the `did_open()` event encode the `text` of the change/document in UTF-8, even though
/// the `Range` (in the case of `did_change()`) that tells you where to apply the change
/// uses UTF-16, so that's cool. UTF-8 `text` is forced to come through due to how the
/// LSP specification uses jsonrpc, where the content fields must be 'utf-8' encoded:
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#contentPart
/// This at least means we have a guarantee that the document itself and any updates to
/// it will be encoded in UTF-8, even if the Positions are UTF-16.
///
/// So we need a way to convert the UTF-16 `Position`s to UTF-8 `tree_sitter::Point`s and
/// back. This requires the document itself, and is what the helpers in this file implement.
pub fn get_position_encoding_kind() -> tower_lsp::lsp_types::PositionEncodingKind {
    tower_lsp::lsp_types::PositionEncodingKind::UTF16
}

pub fn lsp_range_from_tree_sitter_range(
    contents: &str,
    line_index: &LineIndex,
    range: tree_sitter::Range,
) -> tower_lsp::lsp_types::Range {
    let start = lsp_position_from_tree_sitter_point(contents, line_index, range.start_point);
    let end = lsp_position_from_tree_sitter_point(contents, line_index, range.end_point);
    tower_lsp::lsp_types::Range::new(start, end)
}

pub fn tree_sitter_range_from_lsp_range(
    contents: &str,
    line_index: &LineIndex,
    range: tower_lsp::lsp_types::Range,
) -> tree_sitter::Range {
    let start_point = tree_sitter_point_from_lsp_position(contents, line_index, range.start);
    let start_byte = byte_offset_from_tree_sitter_point(line_index, start_point);

    let end_point = tree_sitter_point_from_lsp_position(contents, line_index, range.end);
    let end_byte = byte_offset_from_tree_sitter_point(line_index, end_point);

    tree_sitter::Range {
        start_byte,
        end_byte,
        start_point,
        end_point,
    }
}

pub fn tree_sitter_point_from_lsp_position(
    contents: &str,
    line_index: &LineIndex,
    position: lsp_types::Position,
) -> tree_sitter::Point {
    let line = position.line as usize;
    let character = position.character as usize;

    let character = with_line(
        contents,
        line_index,
        line,
        character,
        utf8_offset_from_utf16_offset,
    );

    tree_sitter::Point::new(line, character)
}

pub fn lsp_position_from_tree_sitter_point(
    contents: &str,
    line_index: &LineIndex,
    point: tree_sitter::Point,
) -> lsp_types::Position {
    let line = point.row;
    let character = point.column;

    let character = with_line(
        contents,
        line_index,
        line,
        character,
        utf16_offset_from_utf8_offset,
    );

    let line = line as u32;
    let character = character as u32;

    lsp_types::Position::new(line, character)
}

fn byte_offset_from_tree_sitter_point(line_index: &LineIndex, point: tree_sitter::Point) -> usize {
    let line_start = match line_index.newlines.get(point.row) {
        Some(offset) => *offset,
        None => {
            log::error!(
                "Failed to get line start for line {}. Document has {} lines.",
                point.row,
                line_index.len()
            );
            return 0;
        },
    };

    let line_start_byte: usize = line_start.into();
    line_start_byte + point.column
}

fn with_line<F>(
    contents: &str,
    line_index: &LineIndex,
    line: usize,
    character: usize,
    f: F,
) -> usize
where
    F: FnOnce(&str, usize) -> usize,
{
    let line_start = match line_index.newlines.get(line) {
        Some(offset) => *offset,
        None => {
            let n = line_index.len();
            let line = line + 1;
            let trace = std::backtrace::Backtrace::force_capture();
            log::error!(
                "Requesting line {line} but only {n} lines exist.\n\nDocument:\n{contents}\n\nBacktrace:\n{trace}"
            );
            return 0;
        },
    };

    let line_end = line_index
        .newlines
        .get(line + 1)
        .copied()
        .unwrap_or_else(|| (contents.len() as u32).into());

    let line_start_byte: usize = line_start.into();
    let line_end_byte: usize = line_end.into();

    let line_str = &contents[line_start_byte..line_end_byte];

    f(line_str, character)
}

/// Converts a character offset into a particular line from UTF-16 to UTF-8
fn utf8_offset_from_utf16_offset(x: &str, utf16_offset: usize) -> usize {
    if x.is_ascii() {
        // Fast pass
        return utf16_offset;
    }

    // Initial check, since loop would skip this case
    if utf16_offset == 0 {
        return utf16_offset;
    }

    let mut n = 0;

    // For each `u32` sized `char`, figure out the equivalent size in UTF-16
    // world of that `char`. Once we hit the requested number of `character`s,
    // that means we have indexed into `x` to the correct position, at which
    // point we can take the current bytes based `pos` that marks the start of
    // this `char`, and add on its UTF-8 based size to return an adjusted column
    // offset. We use `==` because I'm fairly certain they should always align
    // exactly, and it would be good to log if that isn't the case.
    for (pos, char) in x.char_indices() {
        n += char.len_utf16();

        if n == utf16_offset {
            return pos + char.len_utf8();
        }
    }

    log::error!("Failed to locate UTF-16 offset of {utf16_offset}. Line: '{x}'.");
    return 0;
}

/// Converts a character offset into a particular line from UTF-8 to UTF-16
fn utf16_offset_from_utf8_offset(x: &str, utf8_offset: usize) -> usize {
    if x.is_ascii() {
        // Fast pass
        return utf8_offset;
    }

    // The UTF-8 -> UTF-16 case is slightly simpler. We just slice into `x`
    // using our existing UTF-8 offset, reencode the slice as a UTF-16 based
    // iterator, and count up the pieces.
    match x.get(..utf8_offset) {
        Some(x) => x.encode_utf16().count(),
        None => {
            let n = x.len();
            log::error!(
                "Tried to take UTF-8 character {utf8_offset}, but only {n} characters exist. Line: '{x}'."
            );
            0
        },
    }
}
