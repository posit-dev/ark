/*
 * lib.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

pub mod comm;
pub mod connection_file;
pub mod error;
pub mod fixtures;
pub mod kernel;
pub mod kernel_dirs;
pub mod kernel_spec;
pub mod language;
pub mod registration_file;
pub mod session;
pub mod socket;
pub mod stream_capture;
pub mod sys;
pub mod wire;

pub use error::Error;
pub type Result<T> = std::result::Result<T, error::Error>;
