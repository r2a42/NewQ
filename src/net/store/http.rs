//! Media over the show network: a small HTTP server on plain `std` sockets,
//! and the client the puller uses.
//!
//! ```text
//! GET /manifest/current     the version this computer has (the primary: the show's)
//! GET /manifest/<id>        a version it keeps
//! GET /object/<sha256>      the bytes, with `Range: bytes=N-` to resume
//! ```
//!
//! The client is our own rather than `ureq`: a fetch must notice a pulled
//! plug (no byte for a while) without a limit on the whole body, which a
//! 100 GB file can't have, and must stop at once when cancelled.

use super::manifest::Manifest;
use super::paths::is_sha;
use super::rate::{self, RateCap};
use crate::web::parse_request;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The longest request head taken.
const MAX_HEAD: usize = 16 * 1024;
/// The longest manifest taken from the network.
const MAX_MANIFEST: usize = 64 * 1024 * 1024;
/// How much of a file is sent at a time.
const SEND_CHUNK: usize = 128 * 1024;

/// What a computer answers for an object.
pub enum Served {
    /// Send this file, which has these bytes.
    File {
        path: PathBuf,
        size: u64,
    },
    /// It was here but has changed since the version was made (the primary's
    /// file was edited): a newer version will follow.
    Changed,
    NotHere,
}

/// What a computer serves: the primary its show's files, a receiver its store.
pub trait Provider: Send + Sync + 'static {
    fn current_manifest(&self) -> Option<Manifest>;
    fn manifest(&self, id: &str) -> Option<Manifest>;
    fn object(&self, sha: &str) -> Served;
}

/// How much the server puts up with.
#[derive(Clone, Copy, Debug)]
pub struct ServeLimits {
    /// Connections served at once; more are closed unanswered.
    pub connections: usize,
    /// How long a request head has, all told, to arrive.
    pub head_deadline: Duration,
    /// How long one write may wait on a client that has stopped reading.
    pub write_timeout: Duration,
    /// Bytes a second for all the files served together (None: as fast as
    /// the network goes). Keeps the primary's disk free for its own show.
    pub rate_cap: Option<u64>,
}

impl Default for ServeLimits {
    fn default() -> ServeLimits {
        ServeLimits {
            connections: 32,
            head_deadline: Duration::from_secs(5),
            write_timeout: Duration::from_secs(30),
            rate_cap: None,
        }
    }
}

/// Counts, for the status panel and the tests.
#[derive(Default)]
pub struct ServerStats {
    pub requests: AtomicUsize,
    pub bytes_sent: AtomicU64,
    pub objects_served: AtomicUsize,
    /// Files being sent now, and the bytes of them still to send.
    sending: AtomicUsize,
    bytes_left: AtomicU64,
    /// Files being sent now, by the computer asking.
    peers: Mutex<HashMap<IpAddr, usize>>,
    /// When a byte was last sent, in milliseconds from `epoch`.
    last_sent_ms: AtomicU64,
    epoch: OnceLock<Instant>,
}

/// Whether files are going out, for the primary's notice that playback may
/// be less smooth meanwhile (ruling R16). Cheap to ask every frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transfers {
    /// Sending now, or within the last few seconds (so the notice doesn't
    /// flicker between one file and the next).
    pub running: bool,
    /// Computers being sent to now.
    pub computers: usize,
    /// Bytes still to send of the files being sent now.
    pub bytes_left: u64,
}

/// How long after the last byte the transfers still count as running.
const STILL_RUNNING: Duration = Duration::from_secs(3);

impl ServerStats {
    fn ms(&self) -> u64 {
        self.epoch.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    pub fn transfers(&self) -> Transfers {
        let sending = self.sending.load(Ordering::SeqCst);
        let last = self.last_sent_ms.load(Ordering::SeqCst);
        let recent = last > 0 && self.ms().saturating_sub(last) < STILL_RUNNING.as_millis() as u64;
        Transfers {
            running: sending > 0 || recent,
            computers: self.peers.lock().len(),
            bytes_left: self.bytes_left.load(Ordering::SeqCst),
        }
    }
}

/// A file being sent, counted in `ServerStats` until dropped.
struct Sending<'a> {
    stats: &'a ServerStats,
    peer: IpAddr,
    left: u64,
}

impl<'a> Sending<'a> {
    fn new(stats: &'a ServerStats, peer: IpAddr, left: u64) -> Sending<'a> {
        stats.sending.fetch_add(1, Ordering::SeqCst);
        stats.bytes_left.fetch_add(left, Ordering::SeqCst);
        *stats.peers.lock().entry(peer).or_default() += 1;
        Sending { stats, peer, left }
    }

    fn sent(&mut self, n: u64) {
        let n = n.min(self.left);
        self.left -= n;
        self.stats.bytes_left.fetch_sub(n, Ordering::SeqCst);
        self.stats.bytes_sent.fetch_add(n, Ordering::SeqCst);
        // At least 1, so 0 stays "never".
        let ms = self.stats.ms().max(1);
        self.stats.last_sent_ms.store(ms, Ordering::SeqCst);
    }
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        self.stats.sending.fetch_sub(1, Ordering::SeqCst);
        self.stats.bytes_left.fetch_sub(self.left, Ordering::SeqCst);
        let mut peers = self.stats.peers.lock();
        if let Some(n) = peers.get_mut(&self.peer) {
            *n -= 1;
            if *n == 0 {
                peers.remove(&self.peer);
            }
        }
    }
}

/// The listening media server. Dropping it stops it.
pub struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub port: u16,
    pub stats: Arc<ServerStats>,
}

impl Server {
    /// Listen on `bind` (port 0 picks one).
    pub fn start(
        bind: SocketAddr,
        provider: Arc<dyn Provider>,
        limits: ServeLimits,
    ) -> Result<Server, String> {
        let listener = TcpListener::bind(bind).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(ServerStats::default());
        stats.ms();
        let cap = Arc::new(limits.rate_cap.map(RateCap::new));
        let active = Arc::new(AtomicUsize::new(0));
        let (stop2, stats2) = (stop.clone(), stats.clone());
        let thread = std::thread::Builder::new()
            .name("media-serve".into())
            .spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    let Ok((stream, peer)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(25));
                        continue;
                    };
                    if active.fetch_add(1, Ordering::SeqCst) >= limits.connections {
                        active.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let (provider, stats, stop) = (provider.clone(), stats2.clone(), stop2.clone());
                    let cap = cap.clone();
                    let place = Place(active.clone());
                    let spawned = std::thread::Builder::new()
                        .name("media-request".into())
                        .spawn(move || {
                            let _place = place;
                            let ctx = Ctx {
                                provider: &*provider,
                                stats: &stats,
                                limits: &limits,
                                stop: &stop,
                                cap: (*cap).as_ref(),
                                peer: peer.ip(),
                            };
                            serve(stream, &ctx);
                        });
                    // A thread that couldn't start gave its place back when
                    // the closure (and `place` in it) was dropped.
                    let _ = spawned;
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Server {
            stop,
            thread: Some(thread),
            port,
            stats,
        })
    }

    pub fn bytes_sent(&self) -> u64 {
        self.stats.bytes_sent.load(Ordering::SeqCst)
    }

    pub fn transfers(&self) -> Transfers {
        self.stats.transfers()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One of the connections allowed at once, given back when dropped.
struct Place(Arc<AtomicUsize>);

impl Drop for Place {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn head(status: u16, extra: &str, length: u64, content_type: &str) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {length}\r\n{extra}Cache-Control: no-store\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

fn text_reply(stream: &mut TcpStream, status: u16, message: &str) {
    let mut out = head(
        status,
        "",
        message.len() as u64,
        "text/plain; charset=utf-8",
    );
    out.extend_from_slice(message.as_bytes());
    let _ = stream.write_all(&out);
}

/// The start (and end, inclusive) a `Range: bytes=N-[M]` asks for.
pub fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let start = a.trim().parse().ok()?;
    let end = match b.trim() {
        "" => None,
        e => Some(e.parse().ok()?),
    };
    if end.is_some_and(|e| e < start) {
        return None;
    }
    Some((start, end))
}

/// What serving one connection needs.
struct Ctx<'a> {
    provider: &'a dyn Provider,
    stats: &'a ServerStats,
    limits: &'a ServeLimits,
    stop: &'a AtomicBool,
    cap: Option<&'a RateCap>,
    peer: IpAddr,
}

fn serve(mut stream: TcpStream, ctx: &Ctx) {
    let Ctx {
        provider,
        stats,
        limits,
        stop,
        ..
    } = *ctx;
    let started = Instant::now();
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(limits.write_timeout));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let req = loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match parse_request(&buf) {
                Some(r) => break r,
                None => return text_reply(&mut stream, 400, "Bad request"),
            }
        }
        if buf.len() > MAX_HEAD {
            return text_reply(&mut stream, 431, "Request too large");
        }
        if started.elapsed() > limits.head_deadline || stop.load(Ordering::Relaxed) {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    };
    stats.requests.fetch_add(1, Ordering::SeqCst);
    if req.method != "GET" {
        return text_reply(&mut stream, 405, "Only GET");
    }
    let json = |stream: &mut TcpStream, m: Option<Manifest>| match m {
        Some(m) => {
            let body = m.to_json();
            let mut out = head(200, "", body.len() as u64, "application/json");
            out.extend_from_slice(body.as_bytes());
            let _ = stream.write_all(&out);
        }
        None => text_reply(stream, 404, "No such version here"),
    };
    let path = req.path.as_str();
    if path == "/manifest/current" {
        return json(&mut stream, provider.current_manifest());
    }
    if let Some(id) = path.strip_prefix("/manifest/") {
        if !is_sha(id) {
            return text_reply(&mut stream, 400, "Not a version id");
        }
        return json(&mut stream, provider.manifest(id));
    }
    let Some(sha) = path.strip_prefix("/object/") else {
        return text_reply(&mut stream, 404, "Not found");
    };
    if !is_sha(sha) {
        return text_reply(&mut stream, 400, "Not an object name");
    }
    let (file, size) = match provider.object(sha) {
        Served::File { path, size } => (path, size),
        Served::Changed => return text_reply(&mut stream, 410, "The file has changed"),
        Served::NotHere => return text_reply(&mut stream, 404, "Not here"),
    };
    let range = match req.headers.get("range") {
        None => None,
        Some(v) => match parse_range(v) {
            Some(r) => Some(r),
            None => return text_reply(&mut stream, 416, "Bad range"),
        },
    };
    let (start, end) = match range {
        None => (0, size.saturating_sub(1)),
        Some((s, _)) if s >= size && size > 0 => {
            let extra = format!("Content-Range: bytes */{size}\r\n");
            let _ = stream.write_all(&head(416, &extra, 0, "text/plain"));
            return;
        }
        Some((s, e)) => (s, e.unwrap_or(u64::MAX).min(size.saturating_sub(1))),
    };
    let length = if size == 0 || start > end {
        0
    } else {
        end - start + 1
    };
    let Ok(mut f) = std::fs::File::open(&file) else {
        return text_reply(&mut stream, 404, "Not here");
    };
    if f.seek(SeekFrom::Start(start)).is_err() {
        return text_reply(&mut stream, 404, "Not here");
    }
    let out = match range {
        Some(_) if size > 0 => {
            let extra = format!("Content-Range: bytes {start}-{end}/{size}\r\n");
            head(206, &extra, length, "application/octet-stream")
        }
        _ => head(200, "", length, "application/octet-stream"),
    };
    if stream.write_all(&out).is_err() {
        return;
    }
    stats.objects_served.fetch_add(1, Ordering::SeqCst);
    let mut sending = Sending::new(stats, ctx.peer, length);
    let mut left = length;
    let mut data = vec![0u8; SEND_CHUNK];
    while left > 0 && !stop.load(Ordering::Relaxed) {
        let want = (left as usize).min(SEND_CHUNK);
        let n = match f.read(&mut data[..want]) {
            // Shorter than it was: the client sees the body end early.
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if !rate::take(ctx.cap, n as u64, stop) || stream.write_all(&data[..n]).is_err() {
            return;
        }
        sending.sent(n as u64);
        left -= n as u64;
    }
    let _ = stream.flush();
}

// ----- The client ---------------------------------------------------------------------------

/// How long the client waits.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// To reach the other computer.
    pub connect: Duration,
    /// With not one byte arriving, before the other computer is taken to be
    /// gone (a pulled plug). Not a limit on the whole transfer.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(5),
            idle: Duration::from_secs(20),
        }
    }
}

/// Why a request didn't work, in words for the person running the show.
#[derive(Clone, Debug, PartialEq)]
pub enum NetError {
    Cancelled,
    /// Couldn't reach it at all.
    Unreachable(String),
    /// It stopped sending.
    Stalled,
    /// The connection broke or the answer made no sense.
    Broken(String),
    /// It answered with this status.
    Status(u16),
}

impl NetError {
    pub fn words(&self) -> String {
        match self {
            NetError::Cancelled => "stopped".into(),
            NetError::Unreachable(e) => format!("couldn't reach the other computer ({e})"),
            NetError::Stalled => "the other computer stopped sending".into(),
            NetError::Broken(e) => format!("the connection broke ({e})"),
            NetError::Status(404) => "the other computer doesn't have it".into(),
            NetError::Status(410) => "the file changed on the other computer".into(),
            NetError::Status(s) => format!("the other computer answered {s}"),
        }
    }
}

/// An answer whose body is still to be read.
pub struct Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    stream: TcpStream,
    leftover: Vec<u8>,
    remaining: Option<u64>,
    idle: Duration,
}

/// How often a waiting read looks at the cancel flag.
const POLL: Duration = Duration::from_millis(200);

impl Response {
    /// The body's length, if it was said.
    pub fn length(&self) -> Option<u64> {
        self.headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
    }

    /// Read some of the body. 0 at its end. A body that ends before its
    /// stated length is `Broken`.
    pub fn read(&mut self, buf: &mut [u8], cancel: &AtomicBool) -> Result<usize, NetError> {
        if self.remaining == Some(0) {
            return Ok(0);
        }
        let cap = self.remaining.map_or(buf.len(), |r| {
            buf.len().min(r.min(usize::MAX as u64) as usize)
        });
        if !self.leftover.is_empty() {
            let n = cap.min(self.leftover.len());
            buf[..n].copy_from_slice(&self.leftover[..n]);
            self.leftover.drain(..n);
            self.took(n);
            return Ok(n);
        }
        let mut waited = Duration::ZERO;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(NetError::Cancelled);
            }
            match self.stream.read(&mut buf[..cap]) {
                Ok(0) if self.remaining.is_some() => {
                    return Err(NetError::Broken("it ended early".into()));
                }
                Ok(n) => {
                    self.took(n);
                    return Ok(n);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    waited += POLL;
                    if waited >= self.idle {
                        return Err(NetError::Stalled);
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(NetError::Broken(e.to_string())),
            }
        }
    }

    fn took(&mut self, n: usize) {
        if let Some(r) = &mut self.remaining {
            *r -= n as u64;
        }
    }

    /// The whole body, up to `max` bytes.
    pub fn read_all(&mut self, max: usize, cancel: &AtomicBool) -> Result<Vec<u8>, NetError> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = self.read(&mut buf, cancel)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() > max {
                return Err(NetError::Broken("the answer is too long".into()));
            }
        }
    }
}

/// `GET path` from `addr` ("host:port"), asking for the bytes from `from` on.
pub fn get(
    addr: &str,
    path: &str,
    from: u64,
    t: &Timeouts,
    cancel: &AtomicBool,
) -> Result<Response, NetError> {
    let addrs: Vec<SocketAddr> = addr
        .to_socket_addrs()
        .map_err(|e| NetError::Unreachable(e.to_string()))?
        .collect();
    let mut last = String::from("no address");
    let mut stream = None;
    for a in addrs {
        if cancel.load(Ordering::Relaxed) {
            return Err(NetError::Cancelled);
        }
        match TcpStream::connect_timeout(&a, t.connect) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let mut stream = stream.ok_or(NetError::Unreachable(last))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(POLL));
    let _ = stream.set_write_timeout(Some(t.idle));
    let range = if from > 0 {
        format!("Range: bytes={from}-\r\n")
    } else {
        String::new()
    };
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{range}Connection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| NetError::Broken(e.to_string()))?;
    // The head.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut waited = Duration::ZERO;
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err(NetError::Broken("the answer's head is too long".into()));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(NetError::Cancelled);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(NetError::Broken("it closed without answering".into())),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                waited = Duration::ZERO;
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                waited += POLL;
                if waited >= t.idle {
                    return Err(NetError::Stalled);
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(NetError::Broken(e.to_string())),
        }
    };
    let text = std::str::from_utf8(&buf[..end])
        .map_err(|_| NetError::Broken("the answer isn't text".into()))?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut sp = status_line.split(' ');
    let (version, code) = (sp.next().unwrap_or(""), sp.next().unwrap_or(""));
    if !version.starts_with("HTTP/1.") {
        return Err(NetError::Broken("not an HTTP answer".into()));
    }
    let status: u16 = code
        .parse()
        .map_err(|_| NetError::Broken("no status".into()))?;
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((n, v)) = l.split_once(':') {
            headers.insert(n.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let leftover = buf[end + 4..].to_vec();
    let mut r = Response {
        status,
        headers,
        stream,
        leftover,
        remaining: None,
        idle: t.idle,
    };
    r.remaining = r.length();
    Ok(r)
}

/// A manifest from another computer: `which` is "current" or an id. Checked
/// (its id must match its content).
pub fn get_manifest(
    addr: &str,
    which: &str,
    t: &Timeouts,
    cancel: &AtomicBool,
) -> Result<Manifest, NetError> {
    let mut r = get(addr, &format!("/manifest/{which}"), 0, t, cancel)?;
    if r.status != 200 {
        return Err(NetError::Status(r.status));
    }
    let body = r.read_all(MAX_MANIFEST, cancel)?;
    Manifest::from_json(&body).ok_or(NetError::Broken("the list of media arrived damaged".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_read() {
        assert_eq!(parse_range("bytes=10-"), Some((10, None)));
        assert_eq!(parse_range("bytes=10-19"), Some((10, Some(19))));
        assert_eq!(parse_range("bytes=19-10"), None);
        assert_eq!(parse_range("bytes=0-1,5-6"), None);
        assert_eq!(parse_range("items=1-"), None);
        assert_eq!(parse_range("bytes=-5"), None);
    }
}
