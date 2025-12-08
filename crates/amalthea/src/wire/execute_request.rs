/*
 * execute_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use anyhow::Context;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::wire::jupyter_message::MessageType;

/// Represents a request from the frontend to execute code
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteRequest {
    /// The code to be executed
    pub code: String,

    /// Whether the code should be executed silently (not shown to the user)
    pub silent: bool,

    /// Whether the code should be stored in history
    pub store_history: bool,

    /// Mapping of user expressions to be evaluated after code is executed.
    /// (TODO: should not be a plain value)
    pub user_expressions: Value,

    /// Whether to allow the kernel to send stdin requests
    pub allow_stdin: bool,

    /// Whether the kernel should discard the execution queue if evaluating the
    /// code results in an error
    pub stop_on_error: bool,

    /// Posit extension
    pub positron: Option<ExecuteRequestPositron>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteRequestPositron {
    pub code_location: Option<JupyterPositronLocation>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JupyterPositronLocation {
    pub uri: String,
    pub range: JupyterPositronRange,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JupyterPositronRange {
    pub start: JupyterPositronPosition,
    pub end: JupyterPositronPosition,
}

/// See https://jupyter-client.readthedocs.io/en/stable/messaging.html#cursor-pos-unicode-note
/// regarding choice of offset in unicode points
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JupyterPositronPosition {
    pub line: u32,
    /// Column offset in unicode points
    pub character: u32,
}

/// Code location with `character` in UTF-8 offset
#[derive(Debug, Clone)]
pub struct CodeLocation {
    pub uri: Url,
    pub start: Position,
    pub end: Position,
}

/// `character` in UTF-8 offset
#[derive(Debug, Clone)]
pub struct Position {
    pub line: u32,
    pub character: usize,
}

impl ExecuteRequest {
    pub fn code_location(&self) -> anyhow::Result<Option<CodeLocation>> {
        let Some(positron) = &self.positron else {
            return Ok(None);
        };

        let Some(location) = &positron.code_location else {
            return Ok(None);
        };

        let uri = Url::parse(&location.uri).context("Failed to parse URI from code location")?;

        // The location maps `self.code` to a range in the document. We'll first
        // do a sanity check that the span dimensions (end - start) match the
        // code extents.
        let span_lines = location.range.end.line - location.range.start.line;

        // For multiline code, the last line's expected length is just `end.character`.
        // For single-line code, the expected length is `end.character - start.character`.
        let expected_last_line_chars = if span_lines == 0 {
            location.range.end.character - location.range.start.character
        } else {
            location.range.end.character
        };

        let code_lines: Vec<&str> = self.code.lines().collect();
        let code_line_count = code_lines.len().saturating_sub(1);

        // Sanity check: `code` conforms exactly to expected number of lines in the span
        if code_line_count != span_lines as usize {
            return Err(anyhow::anyhow!(
                "Line information does not match code line count (expected {}, got {})",
                code_line_count,
                span_lines
            ));
        }

        let last_line_idx = code_lines.len().saturating_sub(1);
        let last_line = code_lines.get(last_line_idx).unwrap_or(&"");
        let last_line = last_line.strip_suffix('\r').unwrap_or(last_line);
        let last_line_chars = last_line.chars().count() as u32;

        // Sanity check: the last line has exactly the expected number of characters
        if last_line_chars != expected_last_line_chars {
            return Err(anyhow::anyhow!(
                "Expected last line to have {expected} characters, got {actual}",
                expected = expected_last_line_chars,
                actual = last_line_chars
            ));
        }

        // Convert start character from unicode code points to UTF-8 bytes
        let character_start =
            unicode_char_to_utf8_offset(&self.code, 0, location.range.start.character)?;

        // End character is start + last line byte length (for single line)
        // or just last line byte length (for multiline, since it's on a new line)
        let last_line_bytes = last_line.len();
        let character_end = if span_lines == 0 {
            character_start + last_line_bytes
        } else {
            last_line_bytes
        };

        let start = Position {
            line: location.range.start.line,
            character: character_start,
        };
        let end = Position {
            line: location.range.end.line,
            character: character_end,
        };

        Ok(Some(CodeLocation { uri, start, end }))
    }
}

/// Converts a character position in unicode scalar values to a UTF-8 byte
/// offset within the specified line.
fn unicode_char_to_utf8_offset(text: &str, line: u32, character: u32) -> anyhow::Result<usize> {
    let target_line = text
        .lines()
        .nth(line as usize)
        .ok_or_else(|| anyhow::anyhow!("Line {line} not found in text"))?;

    unicode_char_to_utf8_offset_in_line(target_line, character)
}

/// Converts a character count in unicode scalar values to a UTF-8 byte count.
fn unicode_char_to_utf8_offset_in_line(line: &str, character: u32) -> anyhow::Result<usize> {
    let line_chars = line.chars().count();
    if character as usize > line_chars {
        return Err(anyhow::anyhow!(
            "Character position {character} exceeds line length ({line_chars})"
        ));
    }

    let byte_offset = line
        .char_indices()
        .nth(character as usize)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(line.len());

    Ok(byte_offset)
}

impl MessageType for ExecuteRequest {
    fn message_type() -> String {
        String::from("execute_request")
    }
}
