/*
 * iopub.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crossbeam::channel::tick;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use log::trace;
use log::warn;

use crate::error::Error;
use crate::events::PositronEvent;
use crate::socket::socket::Socket;
use crate::wire::client_event::ClientEvent;
use crate::wire::comm_close::CommClose;
use crate::wire::comm_msg::CommMsg;
use crate::wire::comm_open::CommOpen;
use crate::wire::display_data::DisplayData;
use crate::wire::execute_error::ExecuteError;
use crate::wire::execute_input::ExecuteInput;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use crate::wire::stream::Stream;
use crate::wire::stream::StreamOutput;
use crate::wire::update_display_data::UpdateDisplayData;

pub struct IOPub {
    /// The underlying IOPub socket
    socket: Socket,

    /// A channel that receives IOPub messages from other threads
    receiver: Receiver<IOPubMessage>,

    /// The current message context; attached to outgoing messages to pair
    /// outputs with the message that caused them.
    shell_context: Arc<Mutex<Option<JupyterHeader>>>,
    control_context: Arc<Mutex<Option<JupyterHeader>>>,

    /// A buffer for the active stdout/stderr stream to batch stream messages
    /// that we send to the frontend, since this can be extremely high traffic.
    buffer: StreamBuffer,
}

/// Enumeration of possible channels that an IOPub message can be associated
/// with.
pub enum IOPubContextChannel {
    Shell,
    Control,
}

/// Enumeration of all messages that can be delivered from the IOPub PUB/SUB
/// socket. These messages generally are created on other threads and then sent
/// via a channel to the IOPub thread.
pub enum IOPubMessage {
    Status(JupyterHeader, IOPubContextChannel, KernelStatus),
    ExecuteResult(ExecuteResult),
    ExecuteError(ExecuteError),
    ExecuteInput(ExecuteInput),
    Stream(StreamOutput),
    Event(PositronEvent),
    CommOpen(CommOpen),
    CommMsgReply(JupyterHeader, CommMsg),
    CommMsgEvent(CommMsg),
    CommClose(String),
    DisplayData(DisplayData),
    UpdateDisplayData(UpdateDisplayData),
    Flush(Flush),
}

/// A special IOPub message used to force a flush of the active stream buffer,
/// optionally waiting on a response that responds once the request has actually
/// been forwarded to the front end.
pub struct Flush {
    pub flush_tx: Option<Sender<()>>,
}

impl IOPub {
    /// Create a new IOPub socket wrapper.
    ///
    /// * `socket` - The ZeroMQ socket that will deliver IOPub messages to
    ///   subscribed clients.
    /// * `receiver` - The receiver channel that will receive IOPub
    ///   messages from other threads.
    pub fn new(
        socket: Socket,
        receiver: Receiver<IOPubMessage>,
        shell_context: Arc<Mutex<Option<JupyterHeader>>>,
        control_context: Arc<Mutex<Option<JupyterHeader>>>,
    ) -> Self {
        let buffer = StreamBuffer::new(Stream::Stdout);

        Self {
            socket,
            receiver,
            shell_context,
            control_context,
            buffer,
        }
    }

    /// Listen for IOPub messages from other threads. Does not return.
    pub fn listen(&mut self) {
        // Begin by emitting the starting state
        self.emit_state(ExecutionState::Starting);

        // Flush the active stream (either stdout or stderr) at regular
        // intervals
        let flush_interval = StreamBuffer::interval().clone();
        let flush_interval = tick(flush_interval);

        loop {
            select! {
                recv(self.receiver) -> message => {
                    match message {
                        Ok(message) => {
                            if let Err(error) = self.process_message(message) {
                                warn!("Error delivering iopub message: {error:?}")
                            }
                        },
                        Err(error) => {
                            warn!("Failed to receive iopub message: {error:?}");
                        },
                    }
                },
                recv(flush_interval) -> message => {
                    match message {
                        Ok(_) => self.flush_stream(),
                        Err(error) => {
                            warn!("Failed to receive flush interval message: {error:?}");
                        }
                    }
                }
            }
        }
    }

    /// Process an IOPub message from another thread.
    fn process_message(&mut self, message: IOPubMessage) -> Result<(), Error> {
        // TODO: Is there a better way to do this?
        // Flush the stream if we are processing anything other than a `Stream`
        // message. Particularly important for `ExecuteError`s where we need to
        // flush any output that may have been emitted by R before the error
        // occurred.
        match &message {
            IOPubMessage::Stream(_) => {},
            _ => self.flush_stream(),
        };

        match message {
            IOPubMessage::Status(context, context_channel, msg) => {
                // When we enter the Busy state as a result of a message, we
                // update the context. Future messages to IOPub name this
                // context in the parent header sent to the client; this makes
                // it possible for the client to associate events/output with
                // their originator without requiring us to thread the values
                // through the stack.
                match (&context_channel, &msg.execution_state) {
                    (IOPubContextChannel::Control, ExecutionState::Busy) => {
                        let mut control_context = self.control_context.lock().unwrap();
                        *control_context = Some(context.clone());
                    },
                    (IOPubContextChannel::Control, ExecutionState::Idle) => {
                        let mut control_context = self.control_context.lock().unwrap();
                        *control_context = None;
                    },
                    (IOPubContextChannel::Control, ExecutionState::Starting) => {
                        // Nothing to do
                    },
                    (IOPubContextChannel::Shell, ExecutionState::Busy) => {
                        let mut shell_context = self.shell_context.lock().unwrap();
                        *shell_context = Some(context.clone());
                    },
                    (IOPubContextChannel::Shell, ExecutionState::Idle) => {
                        let mut shell_context = self.shell_context.lock().unwrap();
                        *shell_context = None;
                    },
                    (IOPubContextChannel::Shell, ExecutionState::Starting) => {
                        // Nothing to do
                    },
                }

                self.send_message_with_header(context, msg)
            },
            IOPubMessage::ExecuteResult(msg) => {
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::ExecuteError(msg) => {
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::ExecuteInput(msg) => {
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::Stream(msg) => self.process_stream_message(msg),
            IOPubMessage::CommOpen(msg) => self.send_message(msg),
            IOPubMessage::CommMsgEvent(msg) => self.send_message(msg),
            IOPubMessage::CommMsgReply(header, msg) => self.send_message_with_header(header, msg),
            IOPubMessage::CommClose(comm_id) => self.send_message(CommClose { comm_id }),
            IOPubMessage::DisplayData(msg) => {
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::UpdateDisplayData(msg) => {
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::Event(msg) => self.send_event(msg),
            IOPubMessage::Flush(msg) => self.process_flush_message(msg),
        }
    }

    /// Send a message using the underlying socket with the given content.
    /// No parent is assumed.
    fn send_message<T: ProtocolMessage>(&self, content: T) -> Result<(), Error> {
        self.send_message_impl(None, content)
    }

    /// Send a message using the underlying socket with the given content. The
    /// parent message is assumed to be the current context.
    fn send_message_with_context<T: ProtocolMessage>(
        &self,
        content: T,
        context_channel: IOPubContextChannel,
    ) -> Result<(), Error> {
        let context = match context_channel {
            IOPubContextChannel::Control => self.control_context.lock().unwrap(),
            IOPubContextChannel::Shell => self.shell_context.lock().unwrap(),
        };
        self.send_message_impl(context.clone(), content)
    }

    /// Send a message using the underlying socket with the given content and
    /// specific header. Used when the parent message is known by the message
    /// sender, typically in comm message replies.
    fn send_message_with_header<T: ProtocolMessage>(
        &self,
        header: JupyterHeader,
        content: T,
    ) -> Result<(), Error> {
        self.send_message_impl(Some(header), content)
    }

    fn send_message_impl<T: ProtocolMessage>(
        &self,
        header: Option<JupyterHeader>,
        content: T,
    ) -> Result<(), Error> {
        let msg = JupyterMessage::<T>::create(content, header, &self.socket.session);
        msg.send(&self.socket)
    }

    /// Send an event
    fn send_event(&self, event: PositronEvent) -> Result<(), Error> {
        let msg = JupyterMessage::<ClientEvent>::create(
            ClientEvent::from(event),
            None,
            &self.socket.session,
        );
        msg.send(&self.socket)
    }

    /// Flushes the active stream, sending along the message if the buffer
    /// wasn't empty. Handles its own errors since we often call this before
    /// sending some other message and we don't want to prevent that from going
    /// through.
    fn flush_stream(&mut self) {
        let Some(message) = self.buffer.flush() else {
            // Nothing to flush
            return;
        };

        let Err(error) = self.send_message_with_context(message, IOPubContextChannel::Shell) else {
            // Message sent successfully
            return;
        };

        let name = match self.buffer.name {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        };

        warn!("Error delivering iopub 'stream' message over '{name}': {error:?}");
    }

    /// Processes a `Stream` message by appending it to the stream buffer
    ///
    /// The buffer will be flushed on the next tick interval unless it is
    /// manually flushed before then.
    ///
    /// If this new message switches streams, then we flush the existing stream
    /// before switching.
    fn process_stream_message(&mut self, message: StreamOutput) -> Result<(), Error> {
        if message.name != self.buffer.name {
            // Swap streams, but flush the existing stream first
            self.flush_stream();
            self.buffer.swap();
        }

        self.buffer.push(&message.text);

        Ok(())
    }

    fn process_flush_message(&mut self, message: Flush) -> Result<(), Error> {
        self.flush_stream();

        if let Some(flush_tx) = message.flush_tx {
            // Notify receiver that we've send along the flush request
            // to the frontend
            flush_tx.send(()).unwrap();
        }

        Ok(())
    }

    /// Emits the given kernel state to the client.
    fn emit_state(&self, state: ExecutionState) {
        trace!("Entering kernel state: {:?}", state);
        if let Err(err) = JupyterMessage::<KernelStatus>::create(
            KernelStatus {
                execution_state: state,
            },
            None,
            &self.socket.session,
        )
        .send(&self.socket)
        {
            warn!("Could not emit kernel's state. {}", err)
        }
    }
}

struct StreamBuffer {
    name: Stream,
    buffer: String,
    last_flush: Instant,
}

impl StreamBuffer {
    fn new(name: Stream) -> Self {
        return StreamBuffer {
            name,
            buffer: String::new(),
            last_flush: Instant::now(),
        };
    }

    fn push(&mut self, message: &str) {
        self.buffer.push_str(message);
    }

    fn flush(&mut self) -> Option<StreamOutput> {
        if self.buffer.is_empty() {
            // Nothing to send, but we tried a flush so reset the instant
            self.last_flush = Instant::now();
            return None;
        }

        let result = StreamOutput {
            name: self.name.clone(),
            text: self.buffer.clone(),
        };

        self.buffer.clear();
        self.last_flush = Instant::now();

        Some(result)
    }

    fn swap(&mut self) {
        // Clearing the buffer on swap is more efficient than going through
        // `new()` because it doesn't reset the buffer capacity.
        self.name = match self.name {
            Stream::Stdout => Stream::Stderr,
            Stream::Stderr => Stream::Stdout,
        };
        self.buffer.clear();
        self.last_flush = Instant::now();
    }

    fn interval() -> &'static Duration {
        static STREAM_BUFFER_INTERVAL: Duration = Duration::from_millis(80);
        &STREAM_BUFFER_INTERVAL
    }
}
