use oak_sources::PackageCacheWriter;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::mpsc::UnboundedSender;

use crate::lsp::main_loop::report_progress;
use crate::lsp::main_loop::Progress;
use crate::lsp::main_loop::ProgressEvent;
use crate::lsp::main_loop::ProgressEventBegin;
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
    writer: PackageCacheWriter,

    event_rx: TokioUnboundedReceiver<PackageSourcesEvent>,
}

impl PackageSourcesState {
    /// Construct a [PackageSourcesState] and its `event_tx` sender
    pub(crate) fn new(writer: PackageCacheWriter) -> (Self, UnboundedSender<PackageSourcesEvent>) {
        // Channels for communication with the package sources event loop
        let (event_tx, event_rx) = unbounded_channel::<PackageSourcesEvent>();
        (Self { writer, event_rx }, event_tx)
    }

    /// Start the event loop
    pub(crate) async fn start(mut self) {
        while let Some(event) = self.next_event().await {
            self.handle_event(event);
        }
    }

    async fn next_event(&mut self) -> Option<PackageSourcesEvent> {
        self.event_rx.recv().await
    }

    fn handle_event(&mut self, event: PackageSourcesEvent) {
        match event {
            PackageSourcesEvent::Populate(populate) => self.handle_populate(populate),
        }
    }

    fn handle_populate(&mut self, populate: Populate) {
        // We don't populate packages concurrently, so we don't need per package ids
        let id = String::from("package-sources");

        report_progress(Progress::new(
            id.clone(),
            ProgressEvent::Begin(ProgressEventBegin::new(format!(
                "Populating {package}",
                package = &populate.package
            ))),
        ));

        self.writer.insert(&populate.package);

        report_progress(Progress::new(id, ProgressEvent::End));
    }
}
