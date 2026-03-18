//
// spawn.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::future::Future;

use crate::channels::IDLE_ANY_TASKS;
use crate::channels::IDLE_TASKS;
use crate::channels::INTERRUPT_TASKS;
use crate::queue::AsyncTaskData;
use crate::queue::BoxFuture;
use crate::queue::QueuedTask;
use crate::queue::TaskStartInfo;

/// An async task to be run on the R thread.
///
/// Construct via `RTask::interrupt`, `RTask::idle`, or `RTask::idle_any_prompt`
/// when spawning from the R thread. Use the `Send` variants
/// (`RTask::send_interrupt`, etc.) when spawning from other threads.
pub enum RTask {
    /// Run at the next interrupt check. Must be spawned from the R thread.
    Interrupt(BoxFuture<'static, ()>),
    /// Run when R is at a top-level idle prompt. Must be spawned from the R thread.
    Idle(BoxFuture<'static, ()>),
    /// Run when R is at any idle prompt (top-level or browser). Must be spawned
    /// from the R thread.
    IdleAnyPrompt(BoxFuture<'static, ()>),
    /// Like `Interrupt`, but can be spawned from any thread.
    SendInterrupt(BoxFuture<'static, ()>),
    /// Like `Idle`, but can be spawned from any thread.
    SendIdle(BoxFuture<'static, ()>),
    /// Like `IdleAnyPrompt`, but can be spawned from any thread.
    SendIdleAnyPrompt(BoxFuture<'static, ()>),
}

impl RTask {
    pub fn interrupt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::Interrupt(Box::pin(fun()))
    }

    pub fn idle<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::Idle(Box::pin(fun()))
    }

    #[allow(unused)]
    pub fn idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::IdleAnyPrompt(Box::pin(fun()))
    }

    pub fn send_interrupt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendInterrupt(Box::pin(fun()))
    }

    pub fn send_idle<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdle(Box::pin(fun()))
    }

    pub fn send_idle_any_prompt<F, Fut>(fun: F) -> Self
    where
        F: FnOnce() -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static,
    {
        RTask::SendIdleAnyPrompt(Box::pin(fun()))
    }
}

/// Spawn an async task on the R thread.
///
/// For `Send` variants (`RTask::send_interrupt`, etc.) this can be called from
/// any thread. Non-`Send` variants must be called from the R thread.
pub fn spawn(task: RTask) {
    let needs_r_thread = matches!(
        task,
        RTask::Interrupt(_) | RTask::Idle(_) | RTask::IdleAnyPrompt(_)
    );
    if needs_r_thread && !crate::thread::on_r_main_thread() {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        panic!("`spawn()` must be called from the R thread, not thread '{name}'");
    }

    match task {
        RTask::Interrupt(fut) | RTask::SendInterrupt(fut) => spawn_interrupt(fut),
        RTask::Idle(fut) | RTask::SendIdle(fut) => spawn_idle(fut),
        RTask::IdleAnyPrompt(fut) | RTask::SendIdleAnyPrompt(fut) => spawn_idle_any(fut),
    }
}

/// Spawn an async task on the R thread at interrupt priority (sync channel).
///
/// The task will be polled during interrupt checks, even while R is computing.
/// Can be called from any thread.
pub fn spawn_interrupt(fut: BoxFuture<'static, ()>) {
    #[cfg(feature = "testing")]
    if stdext::IS_TESTING && !crate::thread::is_r_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        futures::executor::block_on(fut);
        return;
    }

    let tasks_tx = INTERRUPT_TASKS.tx();
    let task = QueuedTask::Async(AsyncTaskData {
        fut,
        tasks_tx: tasks_tx.clone(),
        start_info: TaskStartInfo::new(false),
    });
    tasks_tx.send(task).unwrap();
}

/// Spawn an async task on the R thread at idle priority.
///
/// The task will only be polled when R is at a top-level idle prompt.
/// Can be called from any thread.
pub fn spawn_idle(fut: BoxFuture<'static, ()>) {
    #[cfg(feature = "testing")]
    if stdext::IS_TESTING && !crate::thread::is_r_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        futures::executor::block_on(fut);
        return;
    }

    let tasks_tx = IDLE_TASKS.tx();
    let task = QueuedTask::Async(AsyncTaskData {
        fut,
        tasks_tx: tasks_tx.clone(),
        start_info: TaskStartInfo::new(true),
    });
    tasks_tx.send(task).unwrap();
}

/// Spawn an async task on the R thread at idle-any priority.
///
/// The task will be polled when R is at any idle prompt (top-level or browser).
/// Can be called from any thread.
pub fn spawn_idle_any(fut: BoxFuture<'static, ()>) {
    #[cfg(feature = "testing")]
    if stdext::IS_TESTING && !crate::thread::is_r_initialized() {
        let _lock = harp::fixtures::R_TEST_LOCK.lock();
        futures::executor::block_on(fut);
        return;
    }

    let tasks_tx = IDLE_ANY_TASKS.tx();
    let task = QueuedTask::Async(AsyncTaskData {
        fut,
        tasks_tx: tasks_tx.clone(),
        start_info: TaskStartInfo::new(true),
    });
    tasks_tx.send(task).unwrap();
}
