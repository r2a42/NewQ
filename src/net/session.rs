//! One TCP connection to another computer: a reader thread that turns frames
//! into events for the hub, and a writer thread fed through a bounded queue.
//! Nothing the app needs ever waits on a peer: a send that finds the queue full
//! fails at once (the hub then drops the session as too slow), writes give up
//! after `WRITE_TIMEOUT`, and closing shuts the socket down, which ends a
//! blocked read on every system.

use super::proto::{self, FrameError, Msg};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long a write may take before the peer is taken to be gone.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// How many frames may wait to be written to one peer.
pub const QUEUE: usize = 1024;

/// Which session an event is about (unique for the hub's life).
pub type ConnId = u64;

/// What a session tells the hub.
#[derive(Debug)]
pub enum SessionEvent {
    /// The first frame: the other side's hello.
    Hello(ConnId, proto::Hello),
    Msg(ConnId, Msg),
    /// It ended: why, and (for a NewQ of another protocol) its name and version.
    Closed(ConnId, FrameError),
}

/// The hub's handle on a session.
pub struct Session {
    pub id: ConnId,
    pub peer: SocketAddr,
    /// This end's address (which adapter reaches the peer).
    pub local: Option<SocketAddr>,
    tx: Sender<Arc<Vec<u8>>>,
    /// Frames written ahead of anything else queued (commands, their acks, the
    /// heartbeat): never behind a piece of the show.
    urgent: Sender<Arc<Vec<u8>>>,
    stream: TcpStream,
    closed: Arc<AtomicBool>,
}

impl Session {
    /// Start the reader and writer for `stream`; events go to `events`.
    pub fn start(
        id: ConnId,
        stream: TcpStream,
        events: Sender<super::hub::Event>,
    ) -> std::io::Result<Session> {
        let peer = stream.peer_addr()?;
        let local = stream.local_addr().ok();
        let _ = stream.set_nodelay(true);
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        stream.set_read_timeout(None)?;
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::bounded::<Arc<Vec<u8>>>(QUEUE);
        let (urgent, urgent_rx) = crossbeam_channel::bounded::<Arc<Vec<u8>>>(QUEUE);
        let writer = stream.try_clone()?;
        let reader = stream.try_clone()?;
        {
            let closed = closed.clone();
            std::thread::Builder::new()
                .name(format!("net-write-{id}"))
                .spawn(move || write_loop(writer, urgent_rx, rx, closed))?;
        }
        {
            let closed = closed.clone();
            std::thread::Builder::new()
                .name(format!("net-read-{id}"))
                .spawn(move || read_loop(id, reader, events, closed))?;
        }
        Ok(Session {
            id,
            peer,
            local,
            tx,
            urgent,
            stream,
            closed,
        })
    }

    /// Queue `msg`. False when the session is closed or the peer is so far
    /// behind that its queue is full: the caller drops it.
    pub fn send(&self, msg: &Msg) -> bool {
        self.send_bytes(Arc::new(proto::encode(msg)))
    }

    /// Queue an encoded frame (the same bytes for many peers).
    pub fn send_bytes(&self, frame: Arc<Vec<u8>>) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        match self.tx.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Queue an encoded frame ahead of the rest (see `urgent`).
    pub fn send_urgent(&self, frame: Arc<Vec<u8>>) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        self.urgent.try_send(frame).is_ok()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// End it now: both threads stop, the reader with a `Closed` event.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// End it once what is queued has been written (a refusal's reason), or
    /// after the write timeout.
    pub fn close_after_queue(&self) {
        let stream = self.stream.try_clone();
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new()
            .name("net-close".into())
            .spawn(move || {
                let began = std::time::Instant::now();
                while !tx.is_empty() && began.elapsed() < WRITE_TIMEOUT {
                    std::thread::sleep(Duration::from_millis(20));
                }
                // A moment for the last frame to leave the socket.
                std::thread::sleep(Duration::from_millis(100));
                if let Ok(s) = stream {
                    let _ = s.shutdown(Shutdown::Both);
                }
            });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

fn write_loop(
    mut stream: TcpStream,
    urgent: Receiver<Arc<Vec<u8>>>,
    rx: Receiver<Arc<Vec<u8>>>,
    closed: Arc<AtomicBool>,
) {
    use std::io::Write;
    loop {
        // The urgent ones first, whatever else waits.
        let frame = match urgent.try_recv() {
            Ok(f) => f,
            Err(_) => crossbeam_channel::select! {
                recv(urgent) -> f => match f { Ok(f) => f, Err(_) => break },
                recv(rx) -> f => match f { Ok(f) => f, Err(_) => break },
            },
        };
        if closed.load(Ordering::Relaxed) {
            break;
        }
        if stream.write_all(&frame).is_err() {
            closed.store(true, Ordering::Relaxed);
            let _ = stream.shutdown(Shutdown::Both);
            break;
        }
    }
}

fn read_loop(
    id: ConnId,
    mut stream: TcpStream,
    events: Sender<super::hub::Event>,
    closed: Arc<AtomicBool>,
) {
    use super::hub::Event;
    let mut first = true;
    let why = loop {
        let body = match proto::read_frame(&mut stream) {
            Ok(b) => b,
            Err(e) => break e,
        };
        if first {
            first = false;
            match proto::decode_hello(&body) {
                Ok(h) => {
                    if events
                        .send(Event::Session(SessionEvent::Hello(id, h)))
                        .is_err()
                    {
                        break FrameError::Closed("the hub stopped".into());
                    }
                }
                Err(e) => break e,
            }
            continue;
        }
        match proto::decode(&body) {
            Ok(Msg::Unknown) => {}
            Ok(m) => {
                if events
                    .send(Event::Session(SessionEvent::Msg(id, m)))
                    .is_err()
                {
                    break FrameError::Closed("the hub stopped".into());
                }
            }
            Err(e) => break e,
        }
    };
    closed.store(true, Ordering::Relaxed);
    let _ = stream.shutdown(Shutdown::Both);
    let _ = events.send(Event::Session(SessionEvent::Closed(id, why)));
}
