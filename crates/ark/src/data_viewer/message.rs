//
// message.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

/**
 * Enum representing the different types of messages that can be sent over the
 * Data Viewer comm channel and their associated data. The JSON representation
 * of this enum is a JSON object with a "msg_type" field that contains the
 * message type; the remaining fields are specific to the message type.
 */
use serde::Deserialize;
use serde::Serialize;

use crate::data_viewer::r_data_viewer::DataSet;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum DataViewerMessageRequest {
    // The data viewer is ready to receive data from the runtime.
    Ready(DataViewerRowRequest),
    // The data viewer is requesting more data from the runtime.
    RequestRows(DataViewerRowRequest),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum DataViewerMessageResponse {
    // Initial data is being sent from the runtime to be displayed in the data viewer.
    InitialData(DataViewerRowResponse),

    // Additional data is being sent from the runtime to be displayed in the data viewer.
    ReceiveRows(DataViewerRowResponse),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DataViewerRowRequest {
    pub start_row: usize,
    pub fetch_size: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DataViewerRowResponse {
    pub start_row: usize,
    pub fetch_size: usize,
    pub data: DataSet,
}
