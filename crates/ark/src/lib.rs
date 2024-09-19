//
// lib.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

pub mod analysis;
pub mod browser;
pub mod connections;
pub mod control;
pub mod coordinates;
pub mod dap;
pub mod data_explorer;
pub mod errors;
pub mod fixtures;
pub mod help;
pub mod help_proxy;
pub mod interface;
pub mod json;
pub mod kernel;
pub mod logger;
pub mod logger_hprof;
pub mod lsp;
pub mod modules;
pub mod modules_utils;
pub mod plots;
pub mod r_task;
pub mod request;
pub mod reticulate;
pub mod shell;
pub mod signals;
pub mod srcref;
pub mod start;
pub mod startup;
pub mod strings;
pub mod sys;
pub mod thread;
pub mod traps;
pub mod treesitter;
pub mod ui;
pub mod variables;
pub mod version;
pub mod viewer;

pub(crate) use r_task::r_task;

pub const ARK_VERSION: &str = env!("CARGO_PKG_VERSION");
