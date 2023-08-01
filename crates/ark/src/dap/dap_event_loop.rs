use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::Arc;

use crossbeam::channel::{unbounded, Receiver, Sender};
use mio::{Events, Interest, Poll, Token, Waker};
use mio_misc::channel::{crossbeam_channel_unbounded, CrossbeamSender};
use mio_misc::queue::NotificationQueue;
use mio_misc::NotificationId;
use stdext::unwrap;

pub const CHANNEL_TOKEN: Token = Token(1);
const TCP_TOKEN: Token = Token(2);

// Implements an event loop thread that watches readiness events for a TCP
// stream and crossbeam channels simultaneously. Incoming and outgoing data
// are forwarded to the appropriate destinations.
pub struct DapEventLoop {
    poll: Poll,
    events: Events,
    raw_tcp_stream: std::net::TcpStream,
    tcp_stream: mio::net::TcpStream,
    queue: Arc<NotificationQueue>,
    dap_outgoing_id: NotificationId,
    dap_incoming_reader: Option<BufReader<ReaderChannel>>,
    dap_outgoing_writer: Option<BufWriter<WriterChannel>>,
    bridge_outgoing_rx: Receiver<Vec<u8>>,
    bridge_incoming_tx: Sender<Vec<u8>>,
}

impl DapEventLoop {
    pub fn new(raw_tcp_stream: std::net::TcpStream) -> Self {
        let events = Events::with_capacity(128);
        let poll = Poll::new().unwrap();
        let waker = Arc::new(Waker::new(poll.registry(), CHANNEL_TOKEN).unwrap());
        let queue = Arc::new(mio_misc::queue::NotificationQueue::new(waker));

        // For incoming communication between the DAP and the bridge
        let (bridge_incoming_tx, dap_incoming_rx) = unbounded::<Vec<u8>>();

        // For outgoing communication between the DAP and the bridge
        let dap_outgoing_id = mio_misc::NotificationId::gen_next();
        let (dap_outgoing_tx, bridge_outgoing_rx) =
            crossbeam_channel_unbounded::<Vec<u8>>(queue.clone(), dap_outgoing_id);

        // Wrap the reading and writing halves in types compatible with
        // `BufReader` and `BufWriter` that the DAP server can read from and
        // write to
        let dap_outgoing_writer = BufWriter::new(WriterChannel::new(dap_outgoing_tx));
        let dap_incoming_reader = BufReader::new(ReaderChannel::new(dap_incoming_rx.clone()));

        // Now switch to non-blocking connection for polling
        raw_tcp_stream.set_nonblocking(true).unwrap();
        let mut tcp_stream = mio::net::TcpStream::from_std(raw_tcp_stream.try_clone().unwrap());

        poll.registry()
            .register(&mut tcp_stream, TCP_TOKEN, Interest::READABLE)
            .unwrap();

        Self {
            poll,
            events,
            raw_tcp_stream,
            tcp_stream,
            queue,
            dap_outgoing_id,
            dap_outgoing_writer: Some(dap_outgoing_writer),
            dap_incoming_reader: Some(dap_incoming_reader),
            bridge_incoming_tx,
            bridge_outgoing_rx,
        }
    }

    // Can only be called once as this moves the value
    pub fn dap_streams(&mut self) -> (BufReader<ReaderChannel>, BufWriter<WriterChannel>) {
        (
            self.dap_incoming_reader.take().unwrap(),
            self.dap_outgoing_writer.take().unwrap(),
        )
    }

    pub fn event_loop(&mut self) -> anyhow::Result<()> {
        // We write in blocking mode on the raw stream to keep things simple
        let mut tcp_writer = BufWriter::new(&self.raw_tcp_stream);

        loop {
            self.poll.poll(&mut self.events, None)?;

            for event in &self.events {
                match event.token() {
                    CHANNEL_TOKEN => {
                        if let Some(id) = self.queue.pop() {
                            // Forward outgoing data from the DAP server to the TCP stream
                            if id == self.dap_outgoing_id {
                                let buf = self.bridge_outgoing_rx.recv();
                                let buf = unwrap!(buf, Err(err) => {
                                    log::trace!("DAP: Outgoing channel closed, closing event loop thread: {err}");
                                    return Ok(());
                                });

                                // Write in blocking mode to avoid async complications
                                self.tcp_stream.try_io(|| -> std::io::Result<()> {
                                    self.raw_tcp_stream.set_nonblocking(false)?;
                                    tcp_writer.write_all(&buf)?;
                                    tcp_writer.flush()?;
                                    self.raw_tcp_stream.set_nonblocking(true)?;
                                    Ok(())
                                })?;

                                if let Err(err) = tcp_writer.write(&buf[..]) {
                                    log::error!("DAP: Can't forward outgoing data: {err}");
                                };
                                continue;
                            }

                            log::error!("DAP: Unknown channel poll ID: {id}");
                        }
                    },

                    TCP_TOKEN => {
                        if event.is_writable() {
                            todo!();
                        }
                        loop {
                            // Forward incoming data from TCP stream to the DAP server
                            let mut buf = vec![0; 4096];
                            match self.tcp_stream.read(&mut buf) {
                                Ok(n) => {
                                    if n == 0 {
                                        log::trace!(
                                            "DAP: TCP stream closed, closing event loop thread"
                                        );
                                        return Ok(());
                                    }

                                    let data = &buf[..n];
                                    unwrap!(
                                        self.bridge_incoming_tx.send(Vec::from(data.clone())),
                                        Err(err) => log::error!("DAP: Can't forward incoming data: {err}")
                                    );

                                    break;
                                },

                                Err(err) if would_block(&err) => {
                                    break;
                                },
                                Err(err) if interrupted(&err) => {
                                    continue;
                                },
                                Err(err) => {
                                    log::error!("DAP: Can't read incoming data: {err}");
                                    break;
                                },
                            }
                        }
                    },

                    token => {
                        log::error!("DAP: Unknown poll token: {:?}", token)
                    },
                }
            }
        }
    }
}

fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

// Wraps a Crossbeam channel to make it `Read`able
pub struct ReaderChannel {
    chan: Receiver<Vec<u8>>,
}

impl ReaderChannel {
    pub fn new(chan: Receiver<Vec<u8>>) -> Self {
        Self { chan }
    }
}

impl Read for ReaderChannel {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes = unwrap!(
            self.chan.recv(),
            Err(_) => return Ok(0)
        );
        let n = Read::read(&mut &bytes[..], buf)?;
        Ok(n)
    }
}

// Wraps an mio-crossbeam channel to make it `Write`able
pub struct WriterChannel {
    chan: CrossbeamSender<Vec<u8>>,
}

impl WriterChannel {
    pub fn new(chan: CrossbeamSender<Vec<u8>>) -> Self {
        Self { chan }
    }
}

impl Write for WriterChannel {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Err(_) = self.chan.send(Vec::from(buf)) {
            return Ok(0);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
