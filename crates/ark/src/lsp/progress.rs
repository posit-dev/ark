use tower_lsp::lsp_types;

use crate::lsp::main_loop::AuxiliaryState;

#[derive(Debug)]
pub(crate) struct Progress {
    /// Identifier for the kind of progress being reported
    ///
    /// If we understand correctly, this allows different identifiers to show up as
    /// different spinners that all report progress concurrently
    id: String,

    event: ProgressEvent,
}

impl Progress {
    pub(crate) fn new(id: String, event: ProgressEvent) -> Self {
        Self { id, event }
    }
}

#[derive(Debug)]
pub(crate) enum ProgressEvent {
    Begin(ProgressEventBegin),
    End,
}

#[derive(Debug)]
pub(crate) struct ProgressEventBegin {
    title: String,
}

impl ProgressEventBegin {
    pub(crate) fn new(title: String) -> Self {
        Self { title }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProgressSupport {
    Enabled,
    Disabled,
}

impl AuxiliaryState {
    pub(crate) fn handle_enable_progress(&mut self) {
        log::info!("Enabling work done progress support");
        self.set_progress_support(ProgressSupport::Enabled);
    }

    pub(crate) async fn handle_progress(&self, progress: Progress) {
        if matches!(self.progress_support(), ProgressSupport::Disabled) {
            return;
        }

        let token = lsp_types::ProgressToken::String(format!("ark/progress/{}", progress.id));

        let work_done_progress = match progress.event {
            ProgressEvent::Begin(begin) => {
                tracing::trace!("handle_progress(begin): token {token:?}");

                let result = self
                    .client()
                    .send_request::<lsp_types::request::WorkDoneProgressCreate>(
                        lsp_types::WorkDoneProgressCreateParams {
                            token: token.clone(),
                        },
                    )
                    .await;

                if let Err(error) = result {
                    log::warn!("Client rejected progress token: {error:?}");
                    return;
                };

                lsp_types::WorkDoneProgress::Begin(lsp_types::WorkDoneProgressBegin {
                    title: begin.title,
                    cancellable: None,
                    message: None,
                    percentage: None,
                })
            },
            ProgressEvent::End => {
                tracing::trace!("handle_progress(end): token {token:?}");
                lsp_types::WorkDoneProgress::End(lsp_types::WorkDoneProgressEnd { message: None })
            },
        };

        self.client()
            .send_notification::<lsp_types::notification::Progress>(lsp_types::ProgressParams {
                token,
                value: lsp_types::ProgressParamsValue::WorkDone(work_done_progress),
            })
            .await;
    }
}
