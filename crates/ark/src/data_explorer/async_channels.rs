//
// async_channels.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::collections::HashMap;
use std::sync::Mutex;

use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;

#[derive(Clone)]
pub struct AsyncChannels<T> {
    pub tx: Sender<T>,
    pub rx: Receiver<T>,
}

pub struct AsyncTaskSocket<Input, Output> {
    // Used to send tasks to the main viewer execution thread
    // and receive tasks in the main viewer execution thread
    pub main: AsyncChannels<(String, Input)>,

    // Used to send results to the worker thread
    pub worker: Mutex<HashMap<String, AsyncChannels<Output>>>,
}

impl<Input, Output> Default for AsyncTaskSocket<Input, Output> {
    fn default() -> Self {
        let (tx, rx) = unbounded::<(String, Input)>();
        Self {
            main: AsyncChannels::<(String, Input)> { tx, rx },
            worker: Mutex::new(HashMap::new()),
        }
    }
}

impl<Input, Output> AsyncTaskSocket<Input, Output>
where
    Output: Clone,
{
    pub fn new_worker_channel(&mut self, id: String) -> AsyncChannels<Output> {
        let (tx, rx) = unbounded::<Output>();
        let channels = AsyncChannels { tx, rx };
        self.worker.lock().unwrap().insert(id, channels.clone());
        channels
    }

    pub fn get_worker_channel(&self, id: &String) -> Option<AsyncChannels<Output>> {
        match self.worker.lock().unwrap().get(id) {
            Some(v) => Some(v.clone()),
            None => None,
        }
    }
}
