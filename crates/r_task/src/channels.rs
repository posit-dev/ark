//
// channels.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::sync::LazyLock;
use std::sync::Mutex;

use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;

use crate::queue::QueuedTask;

/// Task channels for interrupt-priority tasks.
/// Processed anytime: at idle AND during R computation via event loop priority.
pub(crate) static INTERRUPT_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Task channels for idle-time tasks.
/// Only processed at the top-level idle prompt.
pub(crate) static IDLE_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Task channels for idle tasks that run at any idle prompt (top-level or browser).
pub(crate) static IDLE_ANY_TASKS: LazyLock<TaskChannels> = LazyLock::new(TaskChannels::new);

/// Manages a pair of crossbeam channels for sending tasks to the R thread.
///
/// The receiver can only be taken once (by the event loop owner: Console or Oak).
pub(crate) struct TaskChannels {
    tx: Sender<QueuedTask>,
    rx: Mutex<Option<Receiver<QueuedTask>>>,
}

impl TaskChannels {
    fn new() -> Self {
        let (tx, rx) = unbounded::<QueuedTask>();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    pub(crate) fn tx(&self) -> Sender<QueuedTask> {
        self.tx.clone()
    }

    fn take_rx(&self) -> Receiver<QueuedTask> {
        let mut rx = self.rx.lock().unwrap();
        rx.take().expect("`take_rx()` can only be called once")
    }
}

/// Returns receivers for sync, idle, and idle-any task channels.
///
/// Can only be called once. Intended for the R thread event loop owner
/// (Console in Ark, or Oak's headless R thread) during init.
pub fn take_receivers() -> (
    Receiver<QueuedTask>,
    Receiver<QueuedTask>,
    Receiver<QueuedTask>,
) {
    (
        INTERRUPT_TASKS.take_rx(),
        IDLE_TASKS.take_rx(),
        IDLE_ANY_TASKS.take_rx(),
    )
}
