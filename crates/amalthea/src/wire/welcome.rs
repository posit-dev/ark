/*
 * welcome.rs
 *
 * Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// An IOPub message used for handshaking by modern clients.
/// See JEP 65: https://github.com/jupyter/enhancement-proposals/pull/65
///
/// Note that this IOPub `Welcome` message is the same basic idea as
/// `ZMQ_XPUB_WELCOME_MSG`, set through `socket.set_xpub_welcome_msg()`,
/// but the JEP committee decided not to use that.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Welcome {
    /// The `subscription` sent to the XPUB socket by the SUB's call
    /// to `socket.set_subscribe(subscription)`. The IOPub XPUB socket
    /// passes this `subscription` back to the IOPub SUB in the `Welcome`
    /// message.
    pub subscription: String,
}

// Message type comes from copying what xeus and jupyter_kernel_test use:
// https://github.com/jupyter-xeus/xeus-zmq/pull/31
// https://github.com/jupyter/jupyter_kernel_test/blob/5f2c65271b48dc95fc75a9585cb1d6db0bb55557/jupyter_kernel_test/__init__.py#L449-L450
impl MessageType for Welcome {
    fn message_type() -> String {
        String::from("iopub_welcome")
    }
}
