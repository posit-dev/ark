//
// mod.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

pub mod backend;
pub mod comm;
pub mod completions;
mod config;
pub mod definitions;
pub mod diagnostics;
pub mod document_context;
pub mod documents;
pub mod encoding;
pub mod events;
pub mod handler;
pub mod handlers;
pub mod help;
pub mod help_topic;
pub mod hover;
pub mod indent;
pub mod indexer;
pub mod main_loop;
pub mod markdown;
pub mod offset;
pub mod references;
pub mod selection_range;
pub mod signature_help;
pub mod state;
pub mod state_handlers;
pub mod statement_range;
pub mod symbols;
pub mod traits;
pub mod util;

// These send LSP messages in a non-async and non-blocking way.
// The LOG level is not timestamped so we're not using it.
macro_rules! log_info {
    ($($arg:tt)+) => ($crate::lsp::_log!(tower_lsp::lsp_types::MessageType::INFO, $($arg)+))
}
macro_rules! log_warn {
    ($($arg:tt)+) => ($crate::lsp::_log!(tower_lsp::lsp_types::MessageType::WARNING, $($arg)+))
}
macro_rules! log_error {
    ($($arg:tt)+) => ($crate::lsp::_log!(tower_lsp::lsp_types::MessageType::ERROR, $($arg)+))
}
macro_rules! _log {
    ($lvl:expr, $($arg:tt)+) => ({
        $crate::lsp::main_loop::log($lvl, format!($($arg)+));
    });
}

pub(crate) use _log;
pub(crate) use log_error;
pub(crate) use log_info;
pub(crate) use log_warn;
pub(crate) use main_loop::publish_diagnostics;
pub(crate) use main_loop::spawn_blocking;
pub(crate) use main_loop::spawn_diagnostics_refresh;
pub(crate) use main_loop::spawn_diagnostics_refresh_all;
