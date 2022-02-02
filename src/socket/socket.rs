/*
 * socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;

pub trait Socket {
    fn connect(
        ctx: &zmq::Context,
        hmac: Option<Hmac<Sha256>>,
        endpoint: String,
    ) -> Result<Self, Error>;
}

pub trait SocketType {
    fn kind() -> zmq::SocketType;
}

impl<T> Socket for T
where
    T: SocketType,
{
    pub fn connect(ctx: &zmq::Context) -> Result<Self, Error> {
        let socket = ctx.socket(T::kind())?;
        socket.bind(&endpoint)?;
        trace!("Binding to shell socket at {}", endpoint);
        thread::spawn(move || {
            Shell::listen(SignedSocket {
                socket: socket,
                hmac: hmac,
            })
        });
        Ok(())
    }
}
