//
// queue.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crossbeam::channel::Sender;
use uuid::Uuid;

// Compared to `futures::BoxFuture`, this doesn't require the future to be `Send`.
// We don't need this bound since the executor runs only on the R thread.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub enum QueuedTask {
    Sync(SyncTaskData),
    Async(AsyncTaskData),
    Parked(Arc<TaskWaker>),
}

pub struct SyncTaskData {
    pub fun: Box<dyn FnOnce() + Send + 'static>,
    pub status_tx: Option<Sender<TaskStatus>>,
    pub start_info: TaskStartInfo,
}

pub struct AsyncTaskData {
    pub fut: BoxFuture<'static, ()>,
    pub tasks_tx: Sender<QueuedTask>,
    pub start_info: TaskStartInfo,
}

#[derive(Clone)]
pub struct TaskWaker {
    pub id: Uuid,
    pub tasks_tx: Sender<QueuedTask>,
    pub start_info: TaskStartInfo,
}

#[derive(Debug)]
pub enum TaskStatus {
    Started,
    Finished(anyhow::Result<()>),
}

#[derive(Clone)]
pub struct TaskStartInfo {
    pub thread_id: std::thread::ThreadId,
    pub thread_name: String,
    pub start_time: std::time::Instant,

    /// Time it took to run the task. Used to record time accumulated while
    /// running an async task in the executor. Optional because elapsed time is
    /// computed more simply from start time in other cases.
    pub elapsed_time: Option<Duration>,

    /// Tracing span for the task
    pub span: tracing::Span,
}

impl QueuedTask {
    pub fn start_info_mut(&mut self) -> Option<&mut TaskStartInfo> {
        match self {
            QueuedTask::Sync(ref mut task) => Some(&mut task.start_info),
            QueuedTask::Async(ref mut task) => Some(&mut task.start_info),
            QueuedTask::Parked(_) => None,
        }
    }
}

// Safety: `AsyncTaskData` contains a `!Send` future but is sent through
// crossbeam channels. This is safe because the future is only ever polled
// on the single R thread.
unsafe impl Send for AsyncTaskData {}

impl std::task::Wake for TaskWaker {
    fn wake(self: Arc<TaskWaker>) {
        let tasks_tx = self.tasks_tx.clone();
        tasks_tx.send(QueuedTask::Parked(self)).unwrap();
    }
}

impl TaskStartInfo {
    pub fn new(idle: bool) -> Self {
        let thread = std::thread::current();
        let thread_id = thread.id();
        let thread_name = thread
            .name()
            .map(|n| n.to_owned())
            .unwrap_or_else(|| format!("{thread_id:?}"))
            .to_owned();

        let start_time = std::time::Instant::now();
        let span = tracing::trace_span!("R task", thread = thread_name, interrupt = !idle,);

        Self {
            thread_id,
            thread_name,
            start_time,
            elapsed_time: None,
            span,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed_time
            .unwrap_or_else(|| self.start_time.elapsed())
    }

    pub fn bump_elapsed(&mut self, duration: Duration) {
        if let Some(ref mut elapsed_time) = self.elapsed_time {
            *elapsed_time += duration;
        }
    }
}
