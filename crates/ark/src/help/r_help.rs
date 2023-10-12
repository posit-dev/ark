//
// r_help.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;

/**
 * The R Help handler (together with the help proxy) provides the server side of
 * Positron's Help panel.
 */
pub struct RHelp {
    comm: CommSocket,
    comm_manager_tx: Sender<CommEvent>,
}

impl RHelp {
    pub fn start(comm: CommSocket, comm_manager_tx: Sender<CommEvent>) {}
}
