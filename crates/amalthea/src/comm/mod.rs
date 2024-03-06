/*
 * mod.rs
 *
 * Copyright (C) 2022-2023 Posit Software, PBC. All rights reserved.
 *
 */

// Until rustfmt respects `@generated` when executing Format on Save, we have to manually
// skip generated comm files to avoid noise while debugging them
// https://github.com/rust-lang/rustfmt/issues/5080

pub mod base_comm;
pub mod comm_channel;
pub mod comm_manager;
#[rustfmt::skip]
pub mod data_explorer_comm;
pub mod event;
#[rustfmt::skip]
pub mod help_comm;
#[rustfmt::skip]
pub mod plot_comm;
pub mod server_comm;
#[rustfmt::skip]
pub mod ui_comm;
#[rustfmt::skip]
pub mod variables_comm;
#[rustfmt::skip]
pub mod connections_comm;
