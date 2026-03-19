//
// lib.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! R-thread task scheduling infrastructure.
//!
//! Provides the channel infrastructure, synchronous blocking `r_task()` call,
//! async task submission, and R-thread identity primitives. Shared by `ark`,
//! `oak_lsp`, and `oak`.

pub(crate) mod channels;
pub(crate) mod queue;
pub(crate) mod r_task;
pub(crate) mod spawn;
pub(crate) mod thread;

pub use channels::take_receivers;
pub use queue::BoxFuture;
pub use queue::QueuedTask;
pub use queue::TaskStartInfo;
pub use queue::TaskStatus;
pub use queue::TaskWaker;
pub use r_task::r_task;
pub use spawn::spawn;
pub use spawn::RTask;
pub use thread::on_main_thread;
pub use thread::set_initialized;
pub use thread::set_main_thread;
pub use thread::set_test_init_hook;
