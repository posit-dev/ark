//
// message.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use serde::Deserialize;
use serde::Serialize;

use crate::variables::variable::Variable;

/**
 * Enum representing the different types of messages that can be sent over the variables comm
 * channel and their associated data. The JSON representation of this enum is a JSON object with a
 * "msg_type" field that contains the message type; the remaining fields are specific to the message
 * type.
 */
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum VariablesMessage {
    /// A message containing a full listing of variables. Can be triggered by the server or by the
    /// client via a 'refresh' message.
    List(VariablesMessageList),

    /// A message containing a list of variables that have been assigned and a list of variables
    /// that have been removed.
    Update(VariablesMessageUpdate),

    /// A message requesting the server to deliver a full listing of variables.
    Refresh,

    /// A message requesting to clear the variables
    Clear(VariablesMessageClear),

    /// A message requesting to delete some variables
    Delete(VariablesMessageDelete),

    /// A message indicating that the server has successfully processed a client
    /// request. Used only for request messages that do not return data.
    Success,

    /// A message containing an error message.
    Error(VariablesMessageError),

    /// A message requesting to inspect a variable
    Inspect(VariablesMessageInspect),

    /// Details about a variable, response to an Inspect message
    Details(VariablesMessageDetails),

    /// A message requesting to view a variable
    View(VariablesMessageView),

    /// Clipboard format
    ClipboardFormat(VariablesMessageClipboardFormat),

    /// Formatted variable
    FormattedVariable(VariablesMessageFormattedVariable),
}

/**
 * The data for the List message, which contains a full listing of variables.
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageList {
    pub variables: Vec<Variable>,
    pub length: usize,
    pub version: u64,
}

/**
 * The data for the Update message.
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageUpdate {
    pub assigned: Vec<Variable>,
    pub removed: Vec<String>,
    pub version: u64,
}

/**
 * The data for the Error message, which contains an error message.
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageError {
    pub message: String,
}

/**
 * The data for the Clear message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageClear {
    pub include_hidden_objects: bool,
}

/**
 * The data for the Delete message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageDelete {
    pub variables: Vec<String>,
}

/**
 * The data for the Inspect message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageInspect {
    pub path: Vec<String>,
}

/**
 * The data for the Details message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageDetails {
    pub path: Vec<String>,
    pub children: Vec<Variable>,
    pub length: usize,
}

/**
 * The data for the View message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageView {
    pub path: Vec<String>,
}

/*
 * The data for the ClipboardFormat message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageClipboardFormat {
    pub path: Vec<String>,
    pub format: String,
}

/**
 * The data for the ClipboardFormat message
 */
#[derive(Debug, Serialize, Deserialize)]
pub struct VariablesMessageFormattedVariable {
    pub format: String,
    pub content: String,
}
