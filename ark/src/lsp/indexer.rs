//
// indexer.rs
//
// Copyright (C) 2022 by RStudio, PBC
//
//

use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::time::Duration;

use notify::RecommendedWatcher;
use notify::RecursiveMode;
use notify::Watcher;
use notify::watcher;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::lsp::logger::log_push;

#[derive(Debug)]
enum Message {
    UpdateWorkspaceFolders(Vec<String>),
    Stop,
}

#[derive(Debug, Default)]
pub(crate) struct WorkspaceIndexer {
    handle: Option<JoinHandle<()>>,
    channel: Option<Sender<Message>>,
}

impl WorkspaceIndexer {

    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn start(&mut self, folders: Vec<String>) {

        // stop an existing thread if we have one
        self.stop();

        // create a channel for communication with the thread
        let (tx, rx) = channel();

        // initialize a new worker and start running
        log_push!("indexer(): spawning task for file watcher");
        let runtime = Handle::current();
        self.handle = Some(runtime.spawn(async move {
            log_push!("indexer(): task has spawned; watching folders {folders:?}");
            run(rx, folders);
        }));

        // save our channel
        self.channel = Some(tx);

    }

    pub fn stop(&mut self) {

        // if we have a thread, send a stop message and then wait for it to stop
        if self.handle.is_some() {

            // send the stop message
            if let Err(error) = self.channel.as_ref().unwrap().send(Message::Stop) {
                // TODO: log error
            }

            // take ownership of the thread, and join it
            self.handle.take().unwrap().abort();

        }

    }
}

fn run(supervisor: Receiver<Message>, folders: Vec<String>) {

    // create channels of communication for watchers
    log_push!("run(): about to create watchers {folders:?}");
    let (tx, rx) = std::sync::mpsc::channel();

    // create a watcher for each workspace folder; note that we save a reference
    // to the craeted watchers so that they are not prematurely dropped, even
    // though we don't use that variable directly
    //
    // TODO: rather than creating a recursive watcher (which might get a deluge
    // of events for files in hidden directories, or directories we don't care about)
    // consider instead only watching specific folders we care about, or manually
    // recursing into the folders we care about
    let _watchers : Vec<RecommendedWatcher> = folders.iter().map(|folder| {
        log_push!("run(): created watcher for folder {:?}", folder);
        let mut watcher = watcher(tx.clone(), Duration::from_secs(1)).unwrap();
        watcher.watch(folder, RecursiveMode::Recursive).unwrap();
        watcher
    }).collect();


    loop {

        // check for message from notify watcher
        match rx.recv_timeout(Duration::from_secs(1)) {

            Ok(value) => {
                log_push!("indexer(): received watcher value {:?}", value);
            }

            Err(error) => {
                // log_push!("indexer(): received watcher error {:?}", error);
            }

        }

        // check for message from supervisor
        match supervisor.try_recv() {

            Ok(message) => {
                log_push!("indexer(): received supervisor messsage {:?}", message);
            }

            Err(error) => {
                // log_push!("indexer(): received supervisor error {:?}", error);
            }
        }
    }

}
