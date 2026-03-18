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

pub mod channels;
pub mod queue;
pub mod r_task;
pub mod spawn;
pub mod thread;

pub use channels::take_receivers;
pub use queue::AsyncTaskData;
pub use queue::BoxFuture;
pub use queue::QueuedTask;
pub use queue::SyncTaskData;
pub use queue::TaskStartInfo;
pub use queue::TaskStatus;
pub use queue::TaskWaker;
pub use r_task::r_task;
pub use spawn::spawn;
pub use spawn::spawn_idle;
pub use spawn::spawn_idle_any;
pub use spawn::spawn_interrupt;
pub use spawn::RTask;
pub use thread::is_r_initialized;
pub use thread::on_r_main_thread;
pub use thread::set_r_initialized;
pub use thread::set_r_main_thread;
pub use thread::set_test_init_hook;
