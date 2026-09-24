//! Requests the simulated editor has sent, and the answers the session
//! observed for them.
//!
//! One [`Requests`] tracks one kind of request and decodes its answers into
//! `T`. Only requests answered synchronously by their main-loop handler are
//! supported: the answer is read right after that handler returns, so no
//! per-event polling of outstanding requests is needed.

use std::collections::VecDeque;
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

/// An answer as the session observed it, right after the request's handler
/// returned.
#[derive(Debug)]
pub struct RequestAnswer<T> {
    pub response: T,

    /// From sending the request until its handler returned, including the time
    /// the request waited behind earlier events.
    pub latency: Duration,

    /// Duration of this request's main-loop handler.
    pub handled: Duration,
}

pub(super) struct Requests<T> {
    /// Tags this tracker's handles so another tracker can reject them.
    id: u64,

    decode: fn(Option<RequestResponse>) -> T,

    /// Answers indexed by [`RequestHandle`], `None` until the request is
    /// handled.
    answers: Vec<Option<RequestAnswer<T>>>,

    /// Requests not yet handled, in send order. The session enqueues them on
    /// the main loop's FIFO channel, so the next one the loop handles is always
    /// at the front and matching an event is O(1).
    pending: VecDeque<Pending>,
}

/// A tracked request whose event is about to be handled, returned by
/// [`Requests::find()`].
pub(super) struct Tracked(Pending);

struct Pending {
    index: usize,
    sent_at: Instant,
    response_rx: TokioUnboundedReceiver<RequestResponse>,

    /// Identifies the request's event by its reply channel. Weak so that a
    /// request the server drops still disconnects `response_rx`.
    response_tx: WeakUnboundedSender<RequestResponse>,
}

impl<T> Requests<T> {
    /// `decode` receives `None` when the server dropped the request without
    /// answering.
    pub(super) fn new(decode: fn(Option<RequestResponse>) -> T) -> Self {
        Self {
            id: NEXT_TRACKER_ID.fetch_add(1, Ordering::Relaxed),
            decode,
            answers: Vec::new(),
            pending: VecDeque::new(),
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
        self.pending.push_back(Pending {
            index,
            sent_at: Instant::now(),
            response_rx,
            response_tx: response_tx.downgrade(),
        });

        RequestHandle {
            tracker: self.id,
            index,
            answer: PhantomData,
        }
    }

    /// Return the observed answer, or `None` until the request is handled.
    /// Panics if `handle` was issued by another tracker.
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

    /// Claim the tracked request whose event carries `response_tx`, if any.
    /// Call before handling the event, which consumes the sender.
    pub(super) fn find(
        &mut self,
        response_tx: &TokioUnboundedSender<RequestResponse>,
    ) -> Option<Tracked> {
        let front = self.pending.front()?;
        let is_front = front
            .response_tx
            .upgrade()
            .is_some_and(|tracked_tx| tracked_tx.same_channel(response_tx));
        if !is_front {
            return None;
        }
        self.pending.pop_front().map(Tracked)
    }

    /// Read the answer to `tracked` after its handler ran for `handled`.
    pub(super) fn collect(&mut self, tracked: Tracked, handled: Duration) {
        let Tracked(mut pending) = tracked;

        let response = match pending.response_rx.try_recv() {
            Ok(response) => Some(response),
            Err(TryRecvError::Disconnected) => None,
            Err(TryRecvError::Empty) => {
                panic!("A tracked request was answered asynchronously, which the harness doesn't poll for")
            },
        };

        self.answers[pending.index] = Some(RequestAnswer {
            response: (self.decode)(response),
            latency: pending.sent_at.elapsed(),
            handled,
        });
    }
}
