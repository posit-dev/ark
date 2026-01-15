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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JupyterPositronPosition {
    pub line: u32,
    /// Column offset in UTF-8 bytes
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
    pub character: u32,
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
        let range = &location.range;

        // Validate that range is not inverted
        if range.end.line < range.start.line ||
            (range.end.line == range.start.line && range.end.character < range.start.character)
        {
            return Err(anyhow::anyhow!(
                "Invalid range: end ({}, {}) is before start ({}, {})",
                range.end.line,
                range.end.character,
                range.start.line,
                range.start.character
            ));
        }

        // Validate that the span dimensions match the code extents
        let span_lines = (range.end.line - range.start.line) as usize;
        let code_newlines = self.code.matches('\n').count();

        if code_newlines != span_lines {
            return Err(anyhow::anyhow!(
                "Line count mismatch: location spans {span_lines} lines, but code has {code_newlines} newlines"
            ));
        }

        // Validate last line byte length
        let last_line = if self.code.ends_with('\n') {
            ""
        } else {
            self.code.lines().last().unwrap_or("")
        };
        let last_line = last_line.strip_suffix('\r').unwrap_or(last_line);

        let expected_bytes = if span_lines == 0 {
            range.end.character - range.start.character
        } else {
            range.end.character
        };

        if last_line.len() as u32 != expected_bytes {
            return Err(anyhow::anyhow!(
                "Expected last line to have {expected_bytes} bytes, got {}",
                last_line.len()
            ));
        }

        Ok(Some(CodeLocation {
            uri,
            start: Position {
                line: range.start.line,
                character: range.start.character,
            },
            end: Position {
                line: range.end.line,
                character: range.end.character,
            },
        }))
    }
}

impl MessageType for ExecuteRequest {
    fn message_type() -> String {
        String::from("execute_request")
    }
}
