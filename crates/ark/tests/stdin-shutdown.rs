/*
 * stdin-shutdown.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

//! Direct tests for `Stdin::listen` shutdown behaviour. Regression coverage
//! for the issue where dropping the request or interrupt senders left the
//! listen loop spinning on the disconnected channel, spamming error logs
//! during kernel shutdown.

use std::thread;
use std::time::Duration;

use amalthea::session::Session;
use amalthea::socket::stdin::StdInRequest;
use amalthea::socket::stdin::Stdin;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;

/// Spawn `Stdin::listen` on its own thread and return a one-shot receiver
/// that fires after `listen` returns.
fn spawn_listen() -> (
    crossbeam::channel::Sender<StdInRequest>,
    crossbeam::channel::Sender<bool>,
    crossbeam::channel::Receiver<()>,
) {
    let session = Session::create("").unwrap();

    let (_inbound_tx, inbound_rx) = unbounded();
    let (outbound_tx, _outbound_rx) = unbounded();
    let stdin = Stdin::new(inbound_rx, outbound_tx, session);

    let (stdin_request_tx, stdin_request_rx) = unbounded::<StdInRequest>();
    let (stdin_reply_tx, _stdin_reply_rx) = unbounded();
    let (interrupt_tx, interrupt_rx) = bounded::<bool>(1);

    let (done_tx, done_rx) = bounded::<()>(1);
    thread::spawn(move || {
        stdin.listen(stdin_request_rx, stdin_reply_tx, interrupt_rx);
        let _ = done_tx.send(());
    });

    // Keep the inbound and outbound channel endpoints alive for the duration
    // of the test by leaking the receivers/sender we don't return. They're
    // captured by the closure above when the thread reads them; once `listen`
    // exits the thread drops them along with `stdin`.
    (stdin_request_tx, interrupt_tx, done_rx)
}

#[test]
fn test_stdin_exits_when_request_channel_disconnects() {
    let (stdin_request_tx, _interrupt_tx, done_rx) = spawn_listen();

    // Closing all senders for the request channel signals shutdown.
    drop(stdin_request_tx);

    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Stdin::listen did not exit when request channel disconnected");
}

#[test]
fn test_stdin_exits_when_interrupt_channel_disconnects() {
    let (_stdin_request_tx, interrupt_tx, done_rx) = spawn_listen();

    // Closing all senders for the interrupt channel also signals shutdown.
    // Without the fix, the inner select would keep firing on the
    // disconnected interrupt channel and never make progress.
    drop(interrupt_tx);

    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Stdin::listen did not exit when interrupt channel disconnected");
}
