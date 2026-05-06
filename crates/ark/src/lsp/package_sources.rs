use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::mpsc::UnboundedSender;

use crate::lsp::main_loop::TokioUnboundedReceiver;

#[derive(Debug)]
pub(crate) enum PackageSourcesEvent {
    Populate(Populate),
}

#[derive(Debug)]
pub(crate) struct Populate {
    package: String,
}

#[derive(Debug)]
pub(crate) struct PackageSourcesState {
    event_rx: TokioUnboundedReceiver<PackageSourcesEvent>,
}

impl PackageSourcesState {
    /// Construct a [PackageSourcesState] and its `event_tx` sender
    pub(crate) fn new() -> (Self, UnboundedSender<PackageSourcesEvent>) {
        // Channels for communication with the package sources event loop
        let (event_tx, event_rx) = unbounded_channel::<PackageSourcesEvent>();
        (Self { event_rx }, event_tx)
    }

    /// Start the event loop
    pub(crate) async fn start(mut self) {
        while let Some(event) = self.next_event().await {
            self.handle_event(event);
        }
    }

    pub(crate) async fn next_event(&mut self) -> Option<PackageSourcesEvent> {
        self.event_rx.recv().await
    }

    pub(crate) fn handle_event(&mut self, event: PackageSourcesEvent) {
        match event {
            PackageSourcesEvent::Populate(populate) => todo!(),
        }
    }
}
