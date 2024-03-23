/*
 * control.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use nix::sys::signal::Signal;
use nix::sys::signal::{self};
use nix::unistd::Pid;

pub fn handle_interrupt_request() {
    // TODO: Needs to send a SIGINT to the whole process group so that
    // processes started by R will also be interrupted.
    signal::kill(Pid::this(), Signal::SIGINT).unwrap();
}
