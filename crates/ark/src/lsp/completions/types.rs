//
// types.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug)]
pub(super) enum CompletionData {
    DataVariable {
        name: String,
        owner: String,
    },
    Directory {
        path: PathBuf,
    },
    File {
        path: PathBuf,
    },
    Function {
        name: String,
        package: Option<String>,
    },
    Object {
        name: String,
    },
    Keyword {
        name: String,
    },
    Package {
        name: String,
    },
    Parameter {
        name: String,
        function: String,
    },
    RoxygenTag {
        tag: String,
    },
    ScopeParameter {
        name: String,
    },
    ScopeVariable {
        name: String,
    },
    Snippet {
        text: String,
    },
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PromiseStrategy {
    Simple,
    Force,
}
