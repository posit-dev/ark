//! Requests the simulated editor has sent, and the answers the session
//! observed for them.
//!
//! One [`Requests`] tracks one kind of request and decodes its answers into
//! `T` as they are collected.

use std::marker::PhantomData;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::WeakUnboundedSender;

use crate::lsp::backend::RequestResponse;
use crate::lsp::main_loop::TokioUnboundedReceiver;
use crate::lsp::main_loop::TokioUnboundedSender;

/// Source of [`Requests::id`]. Handles can outlive their tracker, and
/// sessions run one after another, so ids must not repeat within the process.
static NEXT_TRACKER_ID: AtomicU64 = AtomicU64::new(0);

/// Identifies a request sent through the session. Typed by the decoded
/// answer so it can only be read back from the tracker that issued it.
pub struct RequestHandle<T> {
    tracker: u64,
    index: usize,
    answer: PhantomData<fn() -> T>,
}

// Derived impls would require `T: Debug`, `T: Clone`, and `T: Copy`.
impl<T> std::fmt::Debug for RequestHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestHandle")
            .field("tracker", &self.tracker)
            .field("index", &self.index)
            .finish()
    }
}

impl<T> Clone for RequestHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for RequestHandle<T> {}

/// An answer as the session observed it.
///
/// Answers are collected after each main-loop handler returns and auxiliary
/// events are recorded, not when the server sends them. Even synchronous
/// replies include that intervening work in their measured latency.
#[derive(Debug)]
pub struct RequestAnswer<T> {
    pub response: T,

    /// From sending the request until the answer was observed, including the
    /// time the request waited behind earlier events.
    pub latency: Duration,

    /// Duration of this request's main-loop handler, whether the handler
    /// answered or handed the work to another thread. `None` if the session
    /// never handled the request's event itself.
    pub handled: Option<Duration>,
}

pub(super) struct Requests<T> {
    /// Tags this tracker's handles so another tracker can reject them.
    id: u64,

    decode: fn(Option<RequestResponse>) -> T,

    /// Answers indexed by [`RequestHandle`], `None` until collected.
    answers: Vec<Option<RequestAnswer<T>>>,

    /// Requests still awaiting an answer. Collection after each event visits
    /// only these, so its cost tracks outstanding requests rather than every
    /// request sent.
    pending: Vec<Pending>,
}

/// A tracked request whose event is about to be handled, returned by
/// [`Requests::find()`] so the handler's duration can be recorded against it.
pub(super) struct Tracked(usize);

struct Pending {
    index: usize,
    sent_at: Instant,
    response_rx: TokioUnboundedReceiver<RequestResponse>,

    /// Identifies the request's event by its reply channel. Weak so that a
    /// request the server drops still disconnects `response_rx`.
    response_tx: WeakUnboundedSender<RequestResponse>,

    handled: Option<Duration>,
}

impl<T> Requests<T> {
    /// `decode` receives `None` when the server dropped the request without
    /// answering.
    pub(super) fn new(decode: fn(Option<RequestResponse>) -> T) -> Self {
        Self {
            id: NEXT_TRACKER_ID.fetch_add(1, Ordering::Relaxed),
            decode,
            answers: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Start tracking a request whose event the caller queues. `response_tx`
    /// is the reply sender carried by that event.
    pub(super) fn track(
        &mut self,
        response_tx: &TokioUnboundedSender<RequestResponse>,
        response_rx: TokioUnboundedReceiver<RequestResponse>,
    ) -> RequestHandle<T> {
        let index = self.answers.len();
        self.answers.push(None);
        self.pending.push(Pending {
            index,
            sent_at: Instant::now(),
            response_rx,
            response_tx: response_tx.downgrade(),
            handled: None,
        });

        RequestHandle {
            tracker: self.id,
            index,
            answer: PhantomData,
        }
    }

    /// Return the observed answer, or `None` until it is collected. Panics if
    /// `handle` was issued by another tracker.
    pub(super) fn answer(&self, handle: RequestHandle<T>) -> Option<&RequestAnswer<T>> {
        if handle.tracker != self.id {
            panic!("{handle:?} was sent by another session");
        }
        self.answers.get(handle.index).and_then(Option::as_ref)
    }

    #[cfg(test)]
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Find the pending request whose event carries `response_tx`. Call before
    /// handling the event, which consumes the sender.
    pub(super) fn find(
        &self,
        response_tx: &TokioUnboundedSender<RequestResponse>,
    ) -> Option<Tracked> {
        self.pending
            .iter()
            .find(|pending| {
                pending
                    .response_tx
                    .upgrade()
                    .is_some_and(|tracked_tx| tracked_tx.same_channel(response_tx))
            })
            .map(|pending| Tracked(pending.index))
    }

    /// Record how long the handler of `tracked` ran. Call before
    /// [`Self::collect()`] so a synchronous answer carries it.
    pub(super) fn record_handled(&mut self, tracked: Tracked, handled: Duration) {
        let Tracked(index) = tracked;
        if let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.index == index)
        {
            pending.handled = Some(handled);
        }
    }

    /// Record every answer that has arrived.
    pub(super) fn collect(&mut self) {
        let observed_at = Instant::now();
        let decode = self.decode;
        let answers = &mut self.answers;

        self.pending.retain_mut(|pending| {
            let response = match pending.response_rx.try_recv() {
                Ok(response) => Some(response),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => None,
            };
            answers[pending.index] = Some(RequestAnswer {
                response: decode(response),
                latency: observed_at - pending.sent_at,
                handled: pending.handled,
            });
            false
        });
    }
}
