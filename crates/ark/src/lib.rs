//
// lib.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

pub mod browser;
pub mod connections;
pub mod control;
pub mod dap;
pub mod data_viewer;
pub mod errors;
pub mod help;
pub mod help_proxy;
pub mod html_widget;
pub mod interface;
pub mod json;
pub mod kernel;
pub mod logger;
pub mod lsp;
pub mod modules;
pub mod plots;
pub mod r_task;
pub mod request;
pub mod shell;
pub mod signals;
pub mod sys;
pub mod test;
pub mod thread;
pub mod traps;
pub mod ui;
pub mod variables;
pub mod version;
pub mod viewer;

pub use r_task::r_task;
