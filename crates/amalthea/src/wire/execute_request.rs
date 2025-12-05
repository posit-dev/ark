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
    pub line: u32,
    pub character: usize,
}

impl ExecuteRequest {
    pub fn extract_code_location(&self) -> anyhow::Result<Option<CodeLocation>> {
        let Some(positron) = &self.positron else {
            return Ok(None);
        };

        let Some(location) = &positron.code_location else {
            return Ok(None);
        };

        let uri = Url::parse(&location.uri).context("Failed to parse URI from code location")?;

        let character = unicode_char_to_utf8_offset(&self.code, 0, location.range.start.character)?;

        Ok(Some(CodeLocation {
            uri,
            line: location.range.start.line,
            character,
        }))
    }
}

/// Converts a character position in unicode scalar values to a UTF-8 byte
/// offset within the specified line.
fn unicode_char_to_utf8_offset(text: &str, line: u32, character: u32) -> anyhow::Result<usize> {
    let target_line = text
        .lines()
        .nth(line as usize)
        .ok_or_else(|| anyhow::anyhow!("Line {line} not found in text"))?;

    let line_chars = target_line.chars().count();
    if character as usize > line_chars {
        return Err(anyhow::anyhow!(
            "Character position {character} exceeds line {line} length ({line_chars})"
        ));
    }

    let byte_offset = target_line
        .char_indices()
        .nth(character as usize)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(target_line.len());

    Ok(byte_offset)
}

impl MessageType for ExecuteRequest {
    fn message_type() -> String {
        String::from("execute_request")
    }
}
