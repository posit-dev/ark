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
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct SocketChannel {
    socket: Arc<Mutex<SignedSocket>>,
    sender: Sender<WireMessage>,
}

impl SocketChannel {
    pub fn new<T: Socket>(
        ctx: &zmq::Context,
        endpoint: String,
        session: Session,
    ) -> Result<Self, Error> {
        let socket = Arc::new(Mutex::new(connect::<T>(ctx, endpoint, session)?));
        let (s, r) = channel::<WireMessage>();
        thread::spawn(move || Self::listen(r, socket.clone()));
        Ok(Self {
            socket: socket,
            sender: s,
        })
    }

    pub fn new_sender(&self) -> Sender<WireMessage> {
        self.sender.clone()
    }

    pub fn read_message(&self) -> Result<Message, Error> {
        Message::read_from_socket(&self.socket.lock().unwrap())
    }

    fn listen(receiver: Receiver<WireMessage>, socket: Arc<Mutex<SignedSocket>>) {
        // TODO error handling
        if let Ok(message) = receiver.recv() {
            message.send(&socket.lock().unwrap());
        }
    }
}
