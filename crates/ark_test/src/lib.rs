//
// lib.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

pub mod accumulator;
pub mod comm;
pub mod dap_assert;
pub mod dap_client;
pub mod dummy_frontend;
pub mod tracing;

// Re-export utilities from ark::fixtures for convenience
pub use accumulator::*;
pub use ark::fixtures::package_is_installed;
pub use ark::fixtures::point_and_offset_from_cursor;
pub use ark::fixtures::point_from_cursor;
pub use ark::fixtures::r_test_init;
pub use ark::fixtures::r_test_lock;
pub use comm::*;
pub use dap_assert::*;
pub use dap_client::*;
pub use dummy_frontend::*;
pub use tracing::*;
