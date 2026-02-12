/*
 * iopub.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::time::Duration;

use crossbeam::channel::tick;
use crossbeam::channel::Receiver;
use crossbeam::channel::Select;
use crossbeam::channel::Sender;
use stdext::result::ResultExt;

use crate::comm::comm_channel::CommMsg;
use crate::session::Session;
use crate::wire::comm_close::CommClose;
use crate::wire::comm_msg::CommWireMsg;
use crate::wire::comm_open::CommOpen;
use crate::wire::display_data::DisplayData;
use crate::wire::execute_error::ExecuteError;
use crate::wire::execute_input::ExecuteInput;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::OutboundMessage;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use crate::wire::stream::Stream;
use crate::wire::stream::StreamOutput;
use crate::wire::subscription_message::SubscriptionKind;
use crate::wire::subscription_message::SubscriptionMessage;
use crate::wire::update_display_data::UpdateDisplayData;
use crate::wire::welcome::Welcome;

pub struct IOPub {
    /// A channel that receives IOPub messages from other threads
    rx: Receiver<IOPubMessage>,

    /// A channel that receives IOPub subscriber notifications from
    /// the IOPub socket
    inbound_rx: Receiver<crate::Result<SubscriptionMessage>>,

    /// A channel to forward along IOPub messages to the IOPub socket
    /// for delivery to the frontend
    outbound_tx: Sender<OutboundMessage>,

    /// A channel that sends a notification when we've received a [SubscriptionMessage],
    /// which ensures that any future IOPub messages sent out from this channel won't be
    /// dropped. We treat this as a one shot channel, and drop it when we've received
    /// the first subscription message, as we only expect one subscriber.
    subscription_tx: Option<Sender<()>>,

    /// ZMQ session used to create messages
    session: Session,

    /// The current message context; attached to outgoing messages to pair
    /// outputs with the message that caused them.
    shell_context: Option<JupyterHeader>,
    control_context: Option<JupyterHeader>,

    /// A buffer for the active stdout/stderr stream to batch stream messages
    /// that we send to the frontend, since this can be extremely high traffic.
    /// We only have 1 buffer because we immediately flush the active stream if
    /// we are about to process a message for the other stream. The idea is that
    /// this avoids a message sequence of <stdout, stderr, stdout> getting
    /// accidentally sent to the frontend as <stdout, stdout, stderr>.
    buffer: StreamBuffer,
}

/// Enumeration of possible channels that an IOPub message can be associated
/// with.
#[derive(Debug)]
pub enum IOPubContextChannel {
    Shell,
    Control,
}

/// Enumeration of all messages that can be delivered from the IOPub XPUB/SUB
/// socket. These messages generally are created on other threads and then sent
/// via a channel to the IOPub thread.
#[derive(Debug)]
pub enum IOPubMessage {
    Status(JupyterHeader, IOPubContextChannel, KernelStatus),
    ExecuteResult(ExecuteResult),
    ExecuteError(ExecuteError),
    ExecuteInput(ExecuteInput),
    Stream(StreamOutput),
    DisplayData(DisplayData),
    UpdateDisplayData(UpdateDisplayData),
    Wait(Wait),
    /// Outgoing comm message from a backend. The String is the comm_id.
    CommOutgoing(String, CommMsg),
}

/// A special IOPub message used to block the sender until the IOPub queue has
/// forwarded all messages before this one on to the frontend.
#[derive(Debug)]
pub struct Wait {
    pub wait_tx: Sender<()>,
}

/// Indexes into the Select for fixed channels.
struct SelectIndex {
    iopub_rx: usize,
    inbound_rx: usize,
    flush_tick: usize,
}

impl IOPub {
    /// Create a new IOPub socket wrapper.
    ///
    /// * `rx` - The receiver channel that will receive IOPub
    ///   messages from other threads.
    /// * `inbound_rx` - The receiver channel that will receive
    ///   new subscriber messages forwarded from the IOPub socket.
    pub fn new(
        rx: Receiver<IOPubMessage>,
        inbound_rx: Receiver<crate::Result<SubscriptionMessage>>,
        outbound_tx: Sender<OutboundMessage>,
        subscription_tx: Sender<()>,
        session: Session,
    ) -> Self {
        let buffer = StreamBuffer::new(Stream::Stdout);

        Self {
            rx,
            inbound_rx,
            outbound_tx,
            subscription_tx: Some(subscription_tx),
            session,
            shell_context: None,
            control_context: None,
            buffer,
        }
    }

    /// Listen for IOPub messages from other threads. Does not return.
    ///
    /// This unified event loop handles:
    /// - IOPub messages from the kernel (status, streams, display data, etc.)
    /// - Subscription messages from the ZMQ socket
    /// - Periodic stream buffer flushing
    pub fn listen(&mut self) {
        let flush_tick = tick(*StreamBuffer::interval());

        loop {
            let mut sel = Select::new();

            // Fixed channels
            let iopub_rx = sel.recv(&self.rx);
            let inbound_rx = sel.recv(&self.inbound_rx);
            let flush_tick_idx = sel.recv(&flush_tick);

            let idx = SelectIndex {
                iopub_rx,
                inbound_rx,
                flush_tick: flush_tick_idx,
            };

            // Block until something is ready
            let oper = sel.select();
            let selected_idx = oper.index();

            // Each branch must consume `oper` by calling `oper.recv()` to release
            // the borrows held by the Select, allowing us to call `&mut self` methods.
            if selected_idx == idx.iopub_rx {
                let msg = match oper.recv(&self.rx) {
                    Ok(msg) => msg,
                    Err(_) => panic!("IOPub message channel disconnected"),
                };
                if let Err(err) = self.process_iopub_message(msg) {
                    log::warn!("Error processing IOPub message: {err:?}");
                }
            } else if selected_idx == idx.inbound_rx {
                let msg = oper.recv(&self.inbound_rx);
                match msg {
                    Ok(Ok(msg)) => {
                        if let Err(err) = self.process_subscription_message(msg) {
                            log::warn!("Error processing subscription message: {err:?}");
                        }
                    },
                    Ok(Err(err)) => {
                        log::warn!("Failed to receive subscription message: {err:?}");
                    },
                    Err(err) => {
                        log::warn!("Subscription channel closed: {err:?}");
                    },
                }
            } else if selected_idx == idx.flush_tick {
                let _ = oper.recv(&flush_tick);
                self.flush_stream();
            }
        }
    }

    /// Process an outbound IOPub message received from another thread.
    ///
    /// These messages originate from Shell, Control, or comm handlers and are
    /// forwarded to the frontend via the IOPub ZMQ socket.
    fn process_iopub_message(&mut self, message: IOPubMessage) -> crate::Result<()> {
        match message {
            IOPubMessage::Status(context, context_channel, content) => {
                // When we enter the Busy state as a result of a message, we
                // update the context. Future messages to IOPub name this
                // context in the parent header sent to the client; this makes
                // it possible for the client to associate events/output with
                // their originator without requiring us to thread the values
                // through the stack.
                match (&context_channel, &content.execution_state) {
                    (IOPubContextChannel::Control, ExecutionState::Busy) => {
                        self.control_context = Some(context.clone());
                    },
                    (IOPubContextChannel::Control, ExecutionState::Idle) => {
                        self.flush_stream();
                        self.control_context = None;
                    },
                    (IOPubContextChannel::Control, ExecutionState::Starting) => {
                        // Nothing to do
                    },
                    (IOPubContextChannel::Shell, ExecutionState::Busy) => {
                        self.shell_context = Some(context.clone());
                    },
                    (IOPubContextChannel::Shell, ExecutionState::Idle) => {
                        self.flush_stream();
                        self.shell_context = None;
                    },
                    (IOPubContextChannel::Shell, ExecutionState::Starting) => {
                        // Nothing to do
                    },
                }

                self.forward(Message::Status(self.message_with_header(context, content)))
            },
            IOPubMessage::ExecuteResult(content) => {
                self.flush_stream();
                self.forward(Message::ExecuteResult(
                    self.message_with_context(content, IOPubContextChannel::Shell),
                ))
            },
            IOPubMessage::ExecuteError(content) => {
                self.flush_stream();
                self.forward(Message::ExecuteError(
                    self.message_with_context(content, IOPubContextChannel::Shell),
                ))
            },
            IOPubMessage::ExecuteInput(content) => self.forward(Message::ExecuteInput(
                self.message_with_context(content, IOPubContextChannel::Shell),
            )),
            IOPubMessage::Stream(content) => self.process_stream_message(content),
            IOPubMessage::DisplayData(content) => {
                self.flush_stream();
                self.forward(Message::DisplayData(
                    self.message_with_context(content, IOPubContextChannel::Shell),
                ))
            },
            IOPubMessage::UpdateDisplayData(content) => {
                self.flush_stream();
                self.forward(Message::UpdateDisplayData(
                    self.message_with_context(content, IOPubContextChannel::Shell),
                ))
            },
            IOPubMessage::Wait(content) => self.process_wait_request(content),
            IOPubMessage::CommOutgoing(comm_id, comm_msg) => {
                self.flush_stream();
                self.process_comm_outgoing(comm_id, comm_msg);
                Ok(())
            },
        }
    }

    /// As an XPUB socket, the only inbound message that IOPub receives is
    /// a subscription message that notifies us when a SUB subscribes or
    /// unsubscribes.
    ///
    /// When we get a subscription notification, we forward along an IOPub
    /// `Welcome` message back to the SUB, in compliance with JEP 65. Clients
    /// that don't know how to process this `Welcome` message should just ignore it.
    fn process_subscription_message(&mut self, message: SubscriptionMessage) -> crate::Result<()> {
        let subscription = message.subscription;

        match message.kind {
            SubscriptionKind::Subscribe => {
                log::info!(
                    "Received subscribe message on IOPub with subscription '{subscription}'."
                );
                self.confirm_subscription(subscription)
            },
            SubscriptionKind::Unsubscribe => {
                log::info!(
                    "Received unsubscribe message on IOPub with subscription '{subscription}'."
                );
                // We don't do anything on unsubscribes
                return Ok(());
            },
        }
    }

    fn confirm_subscription(&mut self, subscription: String) -> crate::Result<()> {
        let Some(subscription_tx) = &self.subscription_tx else {
            let message = "Received subscription message, but no `subscription_tx` is available to confirm on. Have we already received a subscription message once before?";
            log::error!("{message}");
            return Err(crate::anyhow!("{message}"));
        };

        log::info!("Sending `Welcome` message, `Starting` status, and subscription confirmation");

        // Welcome the SUB, in compliance with JEP 65
        self.forward(Message::Welcome(self.message(Welcome { subscription })))?;

        // Follow up with the `ExecutionState::Starting` state for the kernel, which is
        // sent exactly once. Should be after the `Welcome` message in case the client is
        // waiting on the `Welcome` message to proceed.
        self.forward(Message::Status(self.message(KernelStatus {
            execution_state: ExecutionState::Starting,
        })))?;

        // Notify our subscription receiver that we've got a subscriber
        subscription_tx.send(()).unwrap();

        // Unset since this is a once-per process procedure
        self.subscription_tx = None;

        Ok(())
    }

    /// Process an outgoing message from a comm channel.
    fn process_comm_outgoing(&mut self, comm_id: String, comm_msg: CommMsg) {
        let msg = match comm_msg {
            CommMsg::Open { target_name, data } => {
                // Backend-initiated comm open
                Message::CommOpen(self.message(CommOpen {
                    comm_id,
                    target_name,
                    data,
                }))
            },

            CommMsg::Data(data) => {
                // Event: the comm is emitting data to the frontend without being asked
                Message::CommMsg(self.message(CommWireMsg { comm_id, data }))
            },

            CommMsg::Rpc {
                id: _,
                parent_header,
                data,
            } => {
                // RPC reply: the comm is replying to a frontend request
                let payload = CommWireMsg { comm_id, data };
                Message::CommMsg(self.message_with_header(parent_header, payload))
            },

            CommMsg::Close => Message::CommClose(self.message(CommClose { comm_id })),
        };

        self.forward(msg).log_err();
    }

    /// Create a message using the underlying socket with the given content.
    /// No parent is assumed.
    fn message<T: ProtocolMessage>(&self, content: T) -> JupyterMessage<T> {
        self.message_create(None, content)
    }

    /// Create a message using the underlying socket with the given content. The
    /// parent message is assumed to be the current context.
    fn message_with_context<T: ProtocolMessage>(
        &self,
        content: T,
        context_channel: IOPubContextChannel,
    ) -> JupyterMessage<T> {
        let context = match context_channel {
            IOPubContextChannel::Control => &self.control_context,
            IOPubContextChannel::Shell => &self.shell_context,
        };
        self.message_create(context.clone(), content)
    }

    /// Create a message using the underlying socket with the given content and
    /// specific header. Used when the parent message is known by the message
    /// sender, typically in comm message replies.
    fn message_with_header<T: ProtocolMessage>(
        &self,
        header: JupyterHeader,
        content: T,
    ) -> JupyterMessage<T> {
        self.message_create(Some(header), content)
    }

    fn message_create<T: ProtocolMessage>(
        &self,
        header: Option<JupyterHeader>,
        content: T,
    ) -> JupyterMessage<T> {
        JupyterMessage::<T>::create(content, header, &self.session)
    }

    /// Forward a message on to the actual IOPub socket through the outbound channel
    fn forward(&self, message: Message) -> crate::Result<()> {
        self.outbound_tx
            .send(OutboundMessage::IOPub(message))
            .map_err(|err| crate::Error::SendError(format!("{err:?}")))
    }

    /// Flushes the active stream, sending along the message if the buffer
    /// wasn't empty. Handles its own errors since we often call this before
    /// sending some other message and we don't want to prevent that from going
    /// through.
    fn flush_stream(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let content = self.buffer.drain();

        let message =
            Message::Stream(self.message_with_context(content, IOPubContextChannel::Shell));

        let Err(error) = self.forward(message) else {
            // Message sent successfully
            return;
        };

        let name = match self.buffer.name {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        };

        log::warn!("Error delivering iopub 'stream' message over '{name}': {error:?}");
    }

    /// Processes a `Stream` message by appending it to the stream buffer
    ///
    /// The buffer will be flushed on the next tick interval unless it is
    /// manually flushed before then.
    ///
    /// If this new message switches streams, then we flush the existing stream
    /// before switching.
    fn process_stream_message(&mut self, message: StreamOutput) -> crate::Result<()> {
        if message.name != self.buffer.name {
            // Swap streams, but flush the existing stream first
            self.flush_stream();
            self.buffer = StreamBuffer::new(message.name);
        }

        self.buffer.push(message.text);

        Ok(())
    }

    /// Process a `Wait` request
    ///
    /// Processing the request is simple, we just respond. The actual "wait"
    /// occurred in `iopub_tx` / `iopub_rx` where all other pending messages had
    /// to be send along before we got here.
    ///
    /// Note that this doesn't guarantee that the frontend has received all of
    /// the messages on the IOPub socket in front of this one. So even after
    /// waiting for the queue to empty, it is possible for a message on a
    /// different socket that is sent after waiting to still get processed by
    /// the frontend before the messages we cleared from the IOPub queue.
    fn process_wait_request(&mut self, message: Wait) -> crate::Result<()> {
        message.wait_tx.send(()).log_err();
        Ok(())
    }
}

struct StreamBuffer {
    name: Stream,
    buffer: Vec<String>,
}

impl StreamBuffer {
    fn new(name: Stream) -> Self {
        return StreamBuffer {
            name,
            buffer: Vec::new(),
        };
    }

    fn push(&mut self, message: String) {
        self.buffer.push(message);
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    fn drain(&mut self) -> StreamOutput {
        let text = self.buffer.join("");
        self.buffer.clear();

        StreamOutput {
            name: self.name,
            text,
        }
    }

    fn interval() -> &'static Duration {
        static STREAM_BUFFER_INTERVAL: Duration = Duration::from_millis(80);
        &STREAM_BUFFER_INTERVAL
    }
}
