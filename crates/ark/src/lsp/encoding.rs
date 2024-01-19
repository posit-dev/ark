//
// encoding.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use ropey::Rope;
use tower_lsp::lsp_types::Position;
use tree_sitter::Point;

pub fn convert_tree_sitter_range_to_lsp_range(
    x: &Rope,
    range: tree_sitter::Range,
) -> tower_lsp::lsp_types::Range {
    let start = convert_point_to_position(x, range.start_point);
    let end = convert_point_to_position(x, range.end_point);
    tower_lsp::lsp_types::Range::new(start, end)
}

pub fn convert_position_to_point(x: &Rope, position: Position) -> Point {
    let line = position.line as usize;
    let character = position.character as usize;

    let character = with_line(x, line, character, convert_character_from_utf16_to_utf8);

    Point::new(line, character)
}

pub fn convert_point_to_position(x: &Rope, point: Point) -> Position {
    let line = point.row;
    let character = point.column;

    let character = with_line(x, line, character, convert_character_from_utf8_to_utf16);

    let line = line as u32;
    let character = character as u32;

    Position::new(line, character)
}

fn with_line<F>(x: &Rope, line: usize, character: usize, f: F) -> usize
where
    F: FnOnce(&str, usize) -> usize,
{
    // Empty documents come through as an empty string, which looks like 0 lines (TODO: Confirm this?)
    if x.len_lines() == 0 {
        if line != 0 || character != 0 {
            log::error!("Document is empty, but using position: ({line}, {character})");
        }
        return 0;
    }

    let Some(x) = x.get_line(line) else {
        let n = x.len_lines();
        let x = x.to_string();
        let line = line + 1;
        log::error!("Requesting line {line} but only {n} lines exist. Document: '{x}'.");
        return 0;
    };

    // If the line is fully contained in a single chunk (likely is), use free conversion to `&str`
    if let Some(x) = x.as_str() {
        return f(x, character);
    }

    // Otherwise, use ever so slightly more expensive String materialization of the
    // line spread across chunks
    let x = x.to_string();
    let x = x.as_str();

    f(x, character)
}

/// Converts a character offset into a particular line from UTF-16 to UTF-8
fn convert_character_from_utf16_to_utf8(x: &str, character: usize) -> usize {
    if x.is_ascii() {
        // Fast pass
        return character;
    }

    // Handle first iteration, which wouldn't require UTF-8 / UTF-16 translation
    if character == 0 {
        return character;
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

        if n == character {
            return pos + char.len_utf8();
        }
    }

    log::error!("Failed to locate offset of {character}. Line: '{x}'.");
    return 0;
}

/// Converts a character offset into a particular line from UTF-8 to UTF-16
fn convert_character_from_utf8_to_utf16(x: &str, character: usize) -> usize {
    if x.is_ascii() {
        // Fast pass
        return character;
    }

    // The UTF-8 -> UTF-16 case is slightly simpler. We just slice into `x`
    // using our existing UTF-8 offset, reencode the slice as a UTF-16 based
    // iterator, and count up the pieces.
    match x.get(..character) {
        Some(x) => x.encode_utf16().count(),
        None => {
            let n = x.len();
            log::error!(
                "Tried to take character {character}, but only {n} characters exist. Line: '{x}'."
            );
            0
        },
    }
}
