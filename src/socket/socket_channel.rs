/*
 * socket_channel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::session::Session;
use crate::socket::signed_socket::SignedSocket;
use crate::socket::socket::connect;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;
use crate::wire::wire_message::WireMessage;
use log::{trace, warn};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct SocketChannel {
    socket: Arc<Mutex<SignedSocket>>,
    sender: Sender<WireMessage>,
    pub session: Session,
}

impl SocketChannel {
    pub fn new<T: Socket>(
        ctx: &zmq::Context,
        endpoint: String,
        session: Session,
    ) -> Result<Self, Error> {
        let socket = Arc::new(Mutex::new(connect::<T>(ctx, endpoint, session.clone())?));
        let (s, r) = channel::<WireMessage>();
        let socket_clone = socket.clone();
        thread::spawn(move || Self::listen(r, socket_clone));
        Ok(Self {
            socket: socket,
            sender: s,
            session: session,
        })
    }

    pub fn new_sender(&self) -> Sender<WireMessage> {
        self.sender.clone()
    }

    pub fn read_message(&self) -> Result<Message, Error> {
        Message::read_from_socket(&self.socket.lock().unwrap())
    }

    fn listen(receiver: Receiver<WireMessage>, socket: Arc<Mutex<SignedSocket>>) {
        loop {
            trace!("listening for mesages!");
            match receiver.recv() {
                Ok(message) => {
                    trace!("got message {:?}", message);
                    if let Err(err) = message.send(&socket.lock().unwrap()) {
                        warn!("Could not send message to channel: {}", err);
                    }
                }
                Err(err) => {
                    warn!("Could not receive message on channel: {}", err);
                }
            }
        }
    }
}
