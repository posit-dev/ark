//
// lib.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

pub mod comm;
pub mod dap_assert;
pub mod dap_client;
pub mod dummy_frontend;
pub mod log_capture;
pub mod lsp_client;
pub mod tracing;

// Re-export utilities from ark::fixtures and legacy_ide::fixtures for convenience
pub use ark::fixtures::package_is_installed;
pub use comm::*;
pub use dap_assert::*;
pub use dap_client::*;
pub use dummy_frontend::*;
pub use legacy_ide::fixtures::point_and_offset_from_cursor;
pub use legacy_ide::fixtures::point_from_cursor;
pub use log_capture::*;
pub use lsp_client::*;
pub use tracing::*;
