/*
 * stream_capture.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;
use std::os::fd::OwnedFd;

use crossbeam::channel::Sender;
use log::warn;

use crate::error::Error;
use crate::socket::iopub::IOPubMessage;
use crate::wire::stream::Stream;
use crate::wire::stream::StreamOutput;

pub struct StreamCapture {
    iopub_tx: Sender<IOPubMessage>,
}

impl StreamCapture {
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        Self { iopub_tx }
    }

    pub fn listen(&self) {
        if let Err(err) = Self::output_capture(self.iopub_tx.clone()) {
            warn!(
                "Error capturing output; stdout/stderr won't be forwarded: {}",
                err
            );
        };
    }

    /// Captures stdout and stderr streams
    fn output_capture(iopub_tx: Sender<IOPubMessage>) -> Result<(), Error> {
        // Create redirected file descriptors for stdout and stderr. These are
        // pipes into which stdout/stderr are redirected.
        let stdout_fd = Self::redirect_fd(libc::STDOUT_FILENO)?;
        let stderr_fd = Self::redirect_fd(libc::STDERR_FILENO)?;

        // Create poll descriptors for both streams. These are used as
        // arguments to a poll(2) wrapper.
        let stdout_poll = nix::poll::PollFd::new(stdout_fd.as_fd(), nix::poll::PollFlags::POLLIN);
        let stderr_poll = nix::poll::PollFd::new(stderr_fd.as_fd(), nix::poll::PollFlags::POLLIN);
        let mut poll_fds = [stdout_poll, stderr_poll];

        log::info!("Starting thread for stdout/stderr capture");

        loop {
            // Wait for data to be available on either stdout or stderr.  This
            // blocks until data is available, the streams are interrupted, or
            // the timeout occurs.
            let count = match nix::poll::poll(&mut poll_fds, nix::poll::PollTimeout::from(1000u16))
            {
                Ok(c) => c,
                Err(err) => {
                    // https://pubs.opengroup.org/onlinepubs/9699919799/functions/poll.html
                    match err {
                        // If the poll was interrupted, silently continue
                        nix::errno::Errno::EINTR => continue,

                        // Internal allocation has failed, but a retry might succeed
                        nix::errno::Errno::EAGAIN => continue,

                        _ => {
                            log::error!("Error polling for stream data: {err:?}");
                            break;
                        },
                    }
                },
            };

            // No data available; likely timed out waiting for data. Try again.
            if count == 0 {
                continue;
            }

            // See which stream has data available.
            for poll_fd in poll_fds.iter() {
                // Skip this fd if it doesn't have any new events.
                let revents = match poll_fd.revents() {
                    Some(r) => r,
                    None => continue,
                };

                // If the stream has input (POLLIN), read it and send it to the
                // IOPub socket.
                if revents.contains(nix::poll::PollFlags::POLLIN) {
                    let raw_fd = poll_fd.as_fd().as_raw_fd();
                    // Look up the stream name from its file descriptor.
                    let stream = if raw_fd == stdout_fd.as_raw_fd() {
                        Stream::Stdout
                    } else if raw_fd == stderr_fd.as_raw_fd() {
                        Stream::Stderr
                    } else {
                        log::warn!("Unknown stream fd: {}", raw_fd);
                        continue;
                    };

                    // Read the data from the stream and send it to iopub.
                    Self::fd_to_iopub(poll_fd.as_fd(), stream, iopub_tx.clone());
                }
            }
        }

        log::warn!("Stream capture thread exiting after interrupt");
        Ok(())
    }

    /// Reads data from a file descriptor and sends it to the IOPub socket.
    fn fd_to_iopub(fd: BorrowedFd, stream: Stream, iopub_tx: Sender<IOPubMessage>) {
        // Read up to 1024 bytes from the stream into `buf`
        let mut buf = [0u8; 1024];
        let count = match nix::unistd::read(fd, &mut buf) {
            Ok(count) => count,
            Err(e) => {
                warn!("Error reading stream data: {}", e);
                return;
            },
        };

        // No bytes read? Nothing to send.
        if count == 0 {
            return;
        }

        // Convert the UTF-8 bytes to a string.
        let data = String::from_utf8_lossy(&buf[..count]).to_string();
        let output = StreamOutput {
            name: stream,
            text: data,
        };

        // Create and send the IOPub
        let message = IOPubMessage::Stream(output);
        if let Err(e) = iopub_tx.send(message) {
            warn!("Error sending stream data to iopub: {}", e);
        }
    }

    /// Redirects a standard output stream to a pipe and returns the read end of
    /// the pipe.
    fn redirect_fd(fd: i32) -> Result<OwnedFd, Error> {
        // Create a pipe to redirect the stream to
        let (read, write) = match nix::unistd::pipe() {
            Ok((read, write)) => (read, write),
            Err(e) => {
                return Err(Error::SysError(
                    format!("create socket for {}", fd),
                    format!("{e}"),
                ));
            },
        };

        // Redirect the stream into the write end of the pipe.
        // We use `libc::dup2()` directly because nix's `dup2` now requires
        // `&mut OwnedFd` for the target, but STDOUT/STDERR are not owned.
        // Overwriting a global fd is a fundamentally unsafe operation for which
        // nix has no support.
        if unsafe { libc::dup2(write.as_raw_fd(), fd) } == -1 {
            return Err(Error::SysError(
                format!("redirect stream for {}", fd),
                std::io::Error::last_os_error().to_string(),
            ));
        }
        // `write` is dropped at the end of the block, closing the write end of
        // the original pipe. The dup2'd copy on `fd` keeps it alive.

        // Make reads non-blocking on the read end of the pipe
        if let Err(e) = nix::fcntl::fcntl(
            read.as_fd(),
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        ) {
            return Err(Error::SysError(
                format!("set non-blocking for {}", fd),
                e.to_string(),
            ));
        }

        // Return the read end of the pipe
        Ok(read)
    }
}
