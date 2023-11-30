/*
 * iopub.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::time::Duration;

use crossbeam::channel::tick;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use log::trace;
use log::warn;

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::comm_close::CommClose;
use crate::wire::comm_msg::CommWireMsg;
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
    CommOpen(CommOpen),
    CommMsgReply(JupyterHeader, CommWireMsg),
    CommMsgEvent(CommWireMsg),
    CommMsgRequest(CommWireMsg),
    CommClose(String),
    DisplayData(DisplayData),
    UpdateDisplayData(UpdateDisplayData),
    Wait(Wait),
}

/// A special IOPub message used to block the sender until the IOPub queue has
/// forwarded all messages before this one on to the frontend.
pub struct Wait {
    pub wait_tx: Sender<()>,
}

impl IOPub {
    /// Create a new IOPub socket wrapper.
    ///
    /// * `socket` - The ZeroMQ socket that will deliver IOPub messages to
    ///   subscribed clients.
    /// * `receiver` - The receiver channel that will receive IOPub
    ///   messages from other threads.
    pub fn new(socket: Socket, receiver: Receiver<IOPubMessage>) -> Self {
        let buffer = StreamBuffer::new(Stream::Stdout);

        Self {
            socket,
            receiver,
            shell_context: None,
            control_context: None,
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
                        Err(_) => unreachable!()
                    }
                }
            }
        }
    }

    /// Process an IOPub message from another thread.
    fn process_message(&mut self, message: IOPubMessage) -> Result<(), Error> {
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

                self.send_message_with_header(context, msg)
            },
            IOPubMessage::ExecuteResult(msg) => {
                self.flush_stream();
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::ExecuteError(msg) => {
                self.flush_stream();
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::ExecuteInput(msg) => {
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::Stream(msg) => self.process_stream_message(msg),
            IOPubMessage::CommOpen(msg) => self.send_message(msg),
            IOPubMessage::CommMsgEvent(msg) => self.send_message(msg),
            IOPubMessage::CommMsgReply(header, msg) => self.send_message_with_header(header, msg),
            IOPubMessage::CommMsgRequest(msg) => self.send_message(msg),
            IOPubMessage::CommClose(comm_id) => self.send_message(CommClose { comm_id }),
            IOPubMessage::DisplayData(msg) => {
                self.flush_stream();
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::UpdateDisplayData(msg) => {
                self.flush_stream();
                self.send_message_with_context(msg, IOPubContextChannel::Shell)
            },
            IOPubMessage::Wait(msg) => self.process_wait_request(msg),
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
            IOPubContextChannel::Control => &self.control_context,
            IOPubContextChannel::Shell => &self.shell_context,
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

    /// Flushes the active stream, sending along the message if the buffer
    /// wasn't empty. Handles its own errors since we often call this before
    /// sending some other message and we don't want to prevent that from going
    /// through.
    fn flush_stream(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let message = self.buffer.drain();

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
    fn process_wait_request(&mut self, message: Wait) -> Result<(), Error> {
        message.wait_tx.send(()).unwrap();
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
            name: self.name.clone(),
            text,
        }
    }

    fn interval() -> &'static Duration {
        static STREAM_BUFFER_INTERVAL: Duration = Duration::from_millis(80);
        &STREAM_BUFFER_INTERVAL
    }
}
