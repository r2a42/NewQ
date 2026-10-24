//! The web remote: a small web server so a phone or tablet on the same network
//! can run the show from a browser, with GO, Stop All, Pause, the playhead and
//! a button for every cue.
//!
//! Requests are turned into the same `Command`s that OSC and MIDI use. The
//! request handling (`parse_request`, `respond`) is plain functions on bytes,
//! so it is tested without a network; the server around it is a thread per
//! connection on plain `std` sockets.
//!
//! It is off unless the show turns it on, an optional PIN guards it, and
//! changes need a header a web page on another site can't send, so a page in
//! the same browser can't press GO behind your back.

use crate::engine::{CueStatus, Engine};
use crate::remote::Command;
use crate::show::Show;
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// The page a browser is given.
pub const PAGE: &str = include_str!("web_page.html");
/// Requests bigger than this are refused.
const MAX_REQUEST: usize = 16 * 1024;

pub struct Request {
    pub method: String,
    /// The path, without any query string.
    pub path: String,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
}

/// Read the request line and headers from the start of `buf`. None if they
/// are not a complete, well-formed HTTP request head.
pub fn parse_request(buf: &[u8]) -> Option<Request> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target, version) = (first.next()?, first.next()?, first.next()?);
    if !version.starts_with("HTTP/1.") || first.next().is_some() || !target.starts_with('/') {
        return None;
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':')?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Some(Request {
        method: method.to_string(),
        path: target.split('?').next().unwrap_or("/").to_string(),
        headers,
    })
}

/// Undo `%20`-style escapes (a cue number may contain a space or a slash).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The command a POST to `path` asks for: `/api/go`, `/api/panic`,
/// `/api/stop_all`, `/api/pause`, `/api/resume`, `/api/next`, `/api/previous`,
/// and `/api/cue/<number>/<fire|go|select|stop|fade>`.
pub fn command_for(path: &str) -> Option<Command> {
    let rest = path.strip_prefix("/api/")?;
    match rest {
        "go" => return Some(Command::Go),
        "panic" => return Some(Command::Panic),
        "stop_all" => return Some(Command::StopAll),
        "pause" => return Some(Command::Pause),
        "resume" => return Some(Command::Resume),
        "next" => return Some(Command::PlayheadNext),
        "previous" => return Some(Command::PlayheadPrevious),
        _ => {}
    }
    let rest = rest.strip_prefix("cue/")?;
    let (number, verb) = rest.rsplit_once('/')?;
    let number = percent_decode(number);
    if number.trim().is_empty() {
        return None;
    }
    Some(match verb {
        "fire" => Command::Fire(number),
        "go" => Command::GoCue(number),
        "select" => Command::Select(number),
        "stop" => Command::Stop(number),
        "fade" => Command::FadeOut(number),
        _ => return None,
    })
}

/// Compare without stopping at the first difference, so a wrong PIN can't be
/// guessed a character at a time from how long the answer takes.
fn same(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Reply {
    fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status,
            content_type,
            body: body.into(),
        }
    }

    fn text(status: u16, message: &str) -> Reply {
        Reply::new(status, "text/plain; charset=utf-8", message.as_bytes())
    }

    /// The bytes to send: status line, headers, body.
    pub fn to_bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            431 => "Request Header Fields Too Large",
            _ => "Error",
        };
        let mut out = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\
             X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\r\n",
            self.status,
            self.content_type,
            self.body.len()
        )
        .into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

/// What to answer a request with, and the command it asks for, if any.
/// `state` is the show's current state as JSON; `pin` is empty for none.
pub fn respond(req: &Request, pin: &str, state: &str) -> (Reply, Option<Command>) {
    let path = req.path.as_str();
    let api = path.starts_with("/api/");
    if api && !pin.is_empty() {
        let given = req.headers.get("x-pin").map_or("", String::as_str);
        if !same(given.as_bytes(), pin.as_bytes()) {
            return (Reply::text(401, "PIN needed"), None);
        }
    }
    match (req.method.as_str(), path) {
        ("GET", "/" | "/index.html") => (
            Reply::new(200, "text/html; charset=utf-8", PAGE.as_bytes()),
            None,
        ),
        ("GET", "/api/state") => (Reply::new(200, "application/json", state.as_bytes()), None),
        ("POST", _) if api => {
            // A page on another site can post to this address, but only with a
            // header it needs permission to send, which it will not get.
            if req.headers.get("x-newq").map(String::as_str) != Some("1") {
                return (Reply::text(403, "Missing X-NewQ header"), None);
            }
            match command_for(path) {
                Some(c) => (
                    Reply::new(200, "application/json", &b"{\"ok\":true}"[..]),
                    Some(c),
                ),
                None => (Reply::text(404, "No such command"), None),
            }
        }
        // A link or image on another page can only GET, so GET never does anything.
        (_, p) if p.starts_with("/api/") && command_for(p).is_some() => {
            (Reply::text(405, "Use POST"), None)
        }
        ("GET", _) => (Reply::text(404, "Not found"), None),
        _ => (Reply::text(405, "Method not allowed"), None),
    }
}

/// The show's state for the page: the playhead, and a line for every cue.
pub fn snapshot_json(show: &Show, engine: &Engine) -> String {
    let next = engine
        .playhead
        .and_then(|id| show.cue_index(id))
        .and_then(|i| show.armed_top_from(i))
        .map(|i| &show.cues[i]);
    let cues: Vec<_> = show
        .cues
        .iter()
        .map(|c| {
            let (status, progress) = match engine.status(c.id) {
                CueStatus::Idle => ("idle", 0.0),
                CueStatus::PreWait { remaining, total } => {
                    ("prewait", 1.0 - remaining / total.max(1e-6))
                }
                CueStatus::Playing { elapsed, duration } => (
                    "playing",
                    duration.map_or(1.0, |d| (elapsed / d.max(1e-6)).clamp(0.0, 1.0)),
                ),
            };
            json!({
                "number": c.number,
                "name": c.name,
                "type": c.kind.cue_type().label(),
                "armed": c.armed,
                "child": c.parent.is_some(),
                "playhead": engine.playhead == Some(c.id),
                "status": status,
                "progress": progress,
            })
        })
        .collect();
    let timecode = engine
        .timecode
        .position()
        .filter(|_| engine.timecode.running())
        .map(|(t, _)| t.to_string());
    json!({
        "show": show.name,
        "paused": engine.paused,
        "next": next.map(|c| json!({ "number": c.number, "name": c.name })),
        "timecode": timecode,
        "cues": cues,
    })
    .to_string()
}

/// This computer's address on the local network, if it has one: what a phone
/// would type. (Connecting a UDP socket sends nothing; it only picks a route.)
pub fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// The listening web server. Dropping it stops it.
pub struct WebServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub port: u16,
}

impl WebServer {
    /// Listen on `port` (0 picks one). `state` is kept up to date by the app;
    /// commands go to `tx`, and `wake` is called to rouse the event loop.
    pub fn start(
        port: u16,
        pin: String,
        state: Arc<Mutex<String>>,
        tx: Sender<Command>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<WebServer, String> {
        let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let pin = Arc::new(pin);
        let thread = std::thread::Builder::new()
            .name("web-remote".into())
            .spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (pin, state, tx, wake) =
                                (pin.clone(), state.clone(), tx.clone(), wake.clone());
                            // One thread each, so a slow client holds up nobody else.
                            let _ = std::thread::Builder::new()
                                .name("web-request".into())
                                .spawn(move || serve(stream, &pin, &state, &tx, &*wake));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(25)),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(WebServer {
            stop,
            thread: Some(thread),
            port,
        })
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Answer one connection.
fn serve(
    mut stream: TcpStream,
    pin: &str,
    state: &Mutex<String>,
    tx: &Sender<Command>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let reply = loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match parse_request(&buf) {
                Some(req) => {
                    let (reply, command) = respond(&req, pin, &state.lock());
                    if let Some(c) = command {
                        let _ = tx.send(c);
                        wake();
                    }
                    break reply;
                }
                None => break Reply::text(400, "Bad request"),
            }
        }
        if buf.len() > MAX_REQUEST {
            break Reply::text(431, "Request too large");
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let _ = stream.write_all(&reply.to_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{ContinueMode, CueType};

    fn req(method: &str, path: &str, headers: &[(&str, &str)]) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn requests_are_read_from_their_head() {
        let r = parse_request(b"GET /api/state?x=1 HTTP/1.1\r\nHost: a\r\nX-Pin:  1234 \r\n\r\n")
            .unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/api/state"));
        assert_eq!(
            r.headers["x-pin"], "1234",
            "names lower-cased, values trimmed"
        );
        assert_eq!(r.headers["host"], "a");
        for bad in [
            &b"GET / HTTP/1.1\r\nHost: a\r\n"[..], // never ends
            b"GET\r\n\r\n",
            b"GET / FTP/1.0\r\n\r\n",
            b"GET nothing HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.1 extra\r\n\r\n",
            b"GET / HTTP/1.1\r\nno colon here\r\n\r\n",
            b"\xff\xfe / HTTP/1.1\r\n\r\n",
            b"",
        ] {
            assert!(
                parse_request(bad).is_none(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        // A body after the head is not part of it.
        assert!(parse_request(b"POST /api/go HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc").is_some());
    }

    #[test]
    fn paths_map_to_commands() {
        let c = command_for;
        assert_eq!(c("/api/go"), Some(Command::Go));
        assert_eq!(c("/api/panic"), Some(Command::Panic));
        assert_eq!(c("/api/stop_all"), Some(Command::StopAll));
        assert_eq!(c("/api/pause"), Some(Command::Pause));
        assert_eq!(c("/api/resume"), Some(Command::Resume));
        assert_eq!(c("/api/next"), Some(Command::PlayheadNext));
        assert_eq!(c("/api/previous"), Some(Command::PlayheadPrevious));
        assert_eq!(c("/api/cue/12/fire"), Some(Command::Fire("12".into())));
        assert_eq!(c("/api/cue/3.5/go"), Some(Command::GoCue("3.5".into())));
        assert_eq!(
            c("/api/cue/Q1a/select"),
            Some(Command::Select("Q1a".into()))
        );
        assert_eq!(c("/api/cue/7/stop"), Some(Command::Stop("7".into())));
        assert_eq!(c("/api/cue/7/fade"), Some(Command::FadeOut("7".into())));
        // Escapes in the number are undone; a bad one is kept as it is.
        assert_eq!(c("/api/cue/1%20a/fire"), Some(Command::Fire("1 a".into())));
        assert_eq!(c("/api/cue/a%2Fb/fire"), Some(Command::Fire("a/b".into())));
        assert_eq!(c("/api/cue/100%/fire"), Some(Command::Fire("100%".into())));
        for none in [
            "/api/",
            "/api/nope",
            "/api/cue/fire",
            "/api/cue//fire",
            "/api/cue/7/explode",
            "/go",
            "/api/cue/7",
        ] {
            assert_eq!(c(none), None, "{none}");
        }
    }

    #[test]
    fn the_page_and_state_are_served() {
        let (page, cmd) = respond(&req("GET", "/", &[]), "", "{}");
        assert_eq!(page.status, 200);
        assert!(cmd.is_none());
        let text = String::from_utf8(page.body).unwrap();
        for needle in ["NewQ remote", "id=\"go\"", "/api/state", "X-NewQ"] {
            assert!(text.contains(needle), "page lacks {needle}");
        }
        let (state, _) = respond(&req("GET", "/api/state", &[]), "", "{\"a\":1}");
        assert_eq!(
            (state.status, state.content_type),
            (200, "application/json")
        );
        assert_eq!(state.body, b"{\"a\":1}");
        assert_eq!(
            respond(&req("GET", "/missing", &[]), "", "{}").0.status,
            404
        );
    }

    #[test]
    fn a_post_needs_the_header_and_get_never_acts() {
        let post = |path, headers: &[(&str, &str)]| respond(&req("POST", path, headers), "", "{}");
        let (r, c) = post("/api/go", &[("x-newq", "1")]);
        assert_eq!((r.status, c), (200, Some(Command::Go)));
        // Without the header, a page on another site could have sent it.
        let (r, c) = post("/api/go", &[]);
        assert_eq!((r.status, c), (403, None));
        let (r, c) = post("/api/go", &[("x-newq", "0")]);
        assert_eq!((r.status, c), (403, None));
        // Unknown commands are refused.
        assert_eq!(post("/api/explode", &[("x-newq", "1")]).0.status, 404);
        // A GET of a command (a link, an image tag) does nothing.
        let (r, c) = respond(&req("GET", "/api/go", &[]), "", "{}");
        assert_eq!((r.status, c), (405, None));
        for method in ["PUT", "DELETE", "OPTIONS", "HEAD"] {
            let (r, c) = respond(&req(method, "/api/go", &[("x-newq", "1")]), "", "{}");
            assert_eq!((r.status, c), (405, None), "{method}");
        }
    }

    #[test]
    fn a_pin_guards_the_api_but_not_the_page() {
        let state =
            |h: &[(&str, &str)]| respond(&req("GET", "/api/state", h), "4821", "{}").0.status;
        assert_eq!(state(&[]), 401);
        assert_eq!(state(&[("x-pin", "0000")]), 401);
        assert_eq!(state(&[("x-pin", "482")]), 401, "a prefix is not enough");
        assert_eq!(state(&[("x-pin", "48211")]), 401);
        assert_eq!(state(&[("x-pin", "4821")]), 200);
        // Commands too, and the PIN check comes before anything else.
        let go = |h: &[(&str, &str)]| respond(&req("POST", "/api/go", h), "4821", "{}");
        assert_eq!(go(&[("x-newq", "1")]).0.status, 401);
        assert_eq!(
            go(&[("x-newq", "1"), ("x-pin", "4821")]).1,
            Some(Command::Go)
        );
        // The page itself carries nothing secret.
        assert_eq!(respond(&req("GET", "/", &[]), "4821", "{}").0.status, 200);
        assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"abc", b"ab"));
        assert!(same(b"", b""));
    }

    #[test]
    fn replies_are_well_formed_http() {
        let bytes = Reply::new(200, "text/plain", &b"hi"[..]).to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.contains("Cache-Control: no-store"));
        assert!(text.ends_with("\r\n\r\nhi"));
        assert!(
            String::from_utf8(Reply::text(403, "x").to_bytes())
                .unwrap()
                .starts_with("HTTP/1.1 403 Forbidden")
        );
    }

    #[test]
    fn the_state_describes_the_cues_the_playhead_and_what_is_playing() {
        let mut show = Show {
            name: "Hamlet".into(),
            ..Show::default()
        };
        let mut ids = Vec::new();
        for (i, ty) in [CueType::Wait, CueType::Wait, CueType::Group, CueType::Wait]
            .into_iter()
            .enumerate()
        {
            let c = show.make_cue(ty, None, i);
            ids.push(c.id);
            show.cues.push(c);
        }
        show.cues[0].continue_mode = ContinueMode::DoNotContinue;
        show.cues[1].armed = false;
        show.cues[3].parent = Some(ids[2]);
        let mut engine = Engine::for_tests();
        engine.playhead = Some(ids[1]);
        engine.fire(&show, ids[0]);
        engine.tick(&show, 1.0);
        let v: serde_json::Value = serde_json::from_str(&snapshot_json(&show, &engine)).unwrap();
        assert_eq!(v["show"], "Hamlet");
        assert_eq!(v["paused"], false);
        assert_eq!(v["timecode"], serde_json::Value::Null);
        // The playhead is on a disarmed cue, so the next to fire is the one after.
        assert_eq!(v["next"]["name"], show.cues[2].name);
        let cues = v["cues"].as_array().unwrap();
        assert_eq!(cues.len(), 4);
        assert_eq!(cues[0]["status"], "playing");
        assert!(cues[0]["progress"].as_f64().unwrap() > 0.0);
        assert_eq!(cues[1]["armed"], false);
        assert_eq!(cues[1]["playhead"], true);
        assert_eq!(cues[1]["status"], "idle");
        assert_eq!(cues[2]["type"], "Group");
        assert_eq!(cues[3]["child"], true);
        assert_eq!(cues[0]["child"], false);
        // Paused, and at the end of the list.
        engine.paused = true;
        engine.playhead = None;
        let v: serde_json::Value = serde_json::from_str(&snapshot_json(&show, &engine)).unwrap();
        assert_eq!(v["paused"], true);
        assert_eq!(v["next"], serde_json::Value::Null);
    }

    /// Send one raw request to a server and read the whole reply.
    fn ask(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn the_server_answers_real_requests_and_passes_commands_on() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let woken = Arc::new(AtomicBool::new(false));
        let w2 = woken.clone();
        let state = Arc::new(Mutex::new(String::from("{\"cues\":[]}")));
        let server = WebServer::start(
            0,
            "9999".into(),
            state.clone(),
            tx,
            Arc::new(move || w2.store(true, Ordering::SeqCst)),
        )
        .unwrap();
        let port = server.port;
        assert_ne!(port, 0);

        let page = ask(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("NewQ remote"));

        // The state needs the PIN, and follows what the app puts in it.
        let no_pin = ask(port, "GET /api/state HTTP/1.1\r\n\r\n");
        assert!(no_pin.starts_with("HTTP/1.1 401"), "{no_pin}");
        let ok = ask(port, "GET /api/state HTTP/1.1\r\nX-Pin: 9999\r\n\r\n");
        assert!(ok.ends_with("{\"cues\":[]}"), "{ok}");
        *state.lock() = "{\"cues\":[1]}".into();
        let again = ask(port, "GET /api/state HTTP/1.1\r\nX-Pin: 9999\r\n\r\n");
        assert!(again.ends_with("{\"cues\":[1]}"));

        // A command arrives on the channel and wakes the loop.
        assert!(!woken.load(Ordering::SeqCst));
        let go = ask(
            port,
            "POST /api/cue/4/fire HTTP/1.1\r\nX-NewQ: 1\r\nX-Pin: 9999\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(go.starts_with("HTTP/1.1 200 OK"), "{go}");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Command::Fire("4".into()))
        );
        assert!(woken.load(Ordering::SeqCst));
        // Refused ones send nothing.
        ask(port, "POST /api/go HTTP/1.1\r\nX-Pin: 9999\r\n\r\n");
        ask(port, "POST /api/go HTTP/1.1\r\nX-NewQ: 1\r\n\r\n");
        ask(port, "GET /api/go HTTP/1.1\r\nX-Pin: 9999\r\n\r\n");
        assert!(rx.try_recv().is_err(), "nothing else was sent");

        // Rubbish and oversized requests are turned away, not fatal.
        assert!(ask(port, "hello there\r\n\r\n").starts_with("HTTP/1.1 400"));
        let big = format!("GET / HTTP/1.1\r\nX-Big: {}\r\n\r\n", "a".repeat(40_000));
        let r = ask(port, &big);
        assert!(
            r.is_empty() || r.starts_with("HTTP/1.1 431"),
            "{}",
            &r[..r.len().min(40)]
        );

        // A client that connects and says nothing holds up nobody else.
        let _idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let t0 = std::time::Instant::now();
        assert!(ask(port, "GET / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 200"));
        assert!(t0.elapsed() < Duration::from_secs(1));

        // Dropping the server closes the port.
        drop(server);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn a_port_in_use_is_reported() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let first = WebServer::start(
            0,
            String::new(),
            Arc::new(Mutex::new(String::new())),
            tx.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        let second = WebServer::start(
            first.port,
            String::new(),
            Arc::new(Mutex::new(String::new())),
            tx,
            Arc::new(|| {}),
        );
        assert!(second.is_err());
    }
}
