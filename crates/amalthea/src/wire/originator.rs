/*
 * originator.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;

#[derive(Debug, Clone)]
pub struct Originator {
    pub zmq_id: Vec<u8>,
    pub header: JupyterHeader,
}

impl<T> From<&JupyterMessage<T>> for Originator {
    fn from(msg: &JupyterMessage<T>) -> Originator {
        Originator {
            zmq_id: msg.zmq_identities[0].clone(),
            header: msg.header.clone(),
        }
    }
}
