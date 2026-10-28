//! Sending messages out for send cues: OSC over UDP and MIDI.
//!
//! The messages are built by plain functions (tested byte for byte, and read
//! back with the same parsers the remote listeners use). Actual sending is on
//! a worker thread, so a slow name lookup or a stuck port can never stall the
//! show; problems come back through `errors`.

use crate::show::{
    MidiKind, MidiOut, MscCommand, NetworkOut, OscOut, ProjectorOut, SendCue, SendTarget,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum OscArg {
    Int(i32),
    Float(f32),
    Text(String),
}

/// Read arguments from text: "1 0.5 hello \"two words\"". Whole numbers are
/// integers, other numbers are floats, anything else is text.
pub fn parse_args(text: &str) -> Vec<OscArg> {
    let mut args = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else { break };
        let (token, quoted) = if first == '"' {
            chars.next();
            let mut t = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                t.push(c);
            }
            (t, true)
        } else {
            let mut t = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                t.push(c);
                chars.next();
            }
            (t, false)
        };
        args.push(if quoted {
            OscArg::Text(token)
        } else if let Ok(i) = token.parse::<i32>() {
            OscArg::Int(i)
        } else if let Some(f) = token.parse::<f32>().ok().filter(|f| f.is_finite()) {
            OscArg::Float(f)
        } else {
            OscArg::Text(token)
        });
    }
    args
}

fn pad_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// An OSC message: the address, the type tags, and the arguments.
pub fn osc_message(address: &str, args: &[OscArg]) -> Vec<u8> {
    let mut out = Vec::new();
    pad_string(&mut out, address);
    let mut tags = String::from(",");
    for a in args {
        tags.push(match a {
            OscArg::Int(_) => 'i',
            OscArg::Float(_) => 'f',
            OscArg::Text(_) => 's',
        });
    }
    pad_string(&mut out, &tags);
    for a in args {
        match a {
            OscArg::Int(i) => out.extend_from_slice(&i.to_be_bytes()),
            OscArg::Float(f) => out.extend_from_slice(&f.to_be_bytes()),
            OscArg::Text(s) => pad_string(&mut out, s),
        }
    }
    out
}

/// The bytes of a MIDI message. Show Control messages are
/// `F0 7F <device> 02 <format> <command> [cue] F7` to all devices.
pub fn midi_bytes(m: &MidiOut) -> Vec<u8> {
    let channel = m.channel & 0x0F;
    let (d1, d2) = (m.data1 & 0x7F, m.data2 & 0x7F);
    match m.kind {
        MidiKind::NoteOn => vec![0x90 | channel, d1, d2],
        MidiKind::NoteOff => vec![0x80 | channel, d1, d2],
        MidiKind::ControlChange => vec![0xB0 | channel, d1, d2],
        MidiKind::ProgramChange => vec![0xC0 | channel, d1],
        MidiKind::ShowControl(command) => {
            let code = match command {
                MscCommand::Go => 0x01,
                MscCommand::Stop => 0x02,
                MscCommand::Resume => 0x03,
                MscCommand::Load => 0x05,
                MscCommand::AllOff => 0x08,
                MscCommand::Reset => 0x0A,
            };
            // Format 01 is "lighting (general)", which desks of all kinds accept.
            let mut v = vec![0xF0, 0x7F, 0x7F, 0x02, 0x01, code];
            if command.takes_cue() {
                v.extend(
                    m.cue
                        .bytes()
                        .filter(|b| b.is_ascii_graphic())
                        .map(|b| b & 0x7F),
                );
            }
            v.push(0xF7);
            v
        }
    }
}

/// What a send cue puts on the wire.
#[derive(Clone, Debug, PartialEq)]
pub enum Outgoing {
    Osc {
        host: String,
        port: u16,
        packet: Vec<u8>,
    },
    Midi {
        port: Option<String>,
        bytes: Vec<u8>,
    },
    /// A PJLink command. Sent from a thread of its own for that projector,
    /// since a projector can take seconds to answer.
    Projector(ProjectorOut),
    /// Bytes over TCP, from a thread of its own for that host.
    Tcp {
        host: String,
        port: u16,
        payload: Vec<u8>,
    },
}

impl Outgoing {
    pub fn from_cue(cue: &SendCue) -> Outgoing {
        match &cue.target {
            SendTarget::Osc(o) => Outgoing::Osc {
                host: o.host.trim().to_string(),
                port: o.port,
                packet: osc_message(o.address.trim(), &parse_args(&o.args)),
            },
            SendTarget::Midi(m) => Outgoing::Midi {
                port: m.port.clone(),
                bytes: midi_bytes(m),
            },
            SendTarget::Projector(p) => Outgoing::Projector(p.clone()),
            SendTarget::Network(NetworkOut {
                host,
                port,
                tcp,
                payload,
            }) => {
                let payload = unescape(payload);
                let host = host.trim().to_string();
                if *tcp {
                    Outgoing::Tcp {
                        host,
                        port: *port,
                        payload,
                    }
                } else {
                    // One UDP packet, like an OSC message with other contents.
                    Outgoing::Osc {
                        host,
                        port: *port,
                        packet: payload,
                    }
                }
            }
        }
    }
}

/// Bytes from text typed with escapes: `\r`, `\n`, `\t`, `\\`, `\0` and `\xNN`
/// (two hex digits). Anything else after a backslash stays as typed.
pub fn unescape(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.peek().copied() {
            Some('r') => {
                chars.next();
                out.push(b'\r');
            }
            Some('n') => {
                chars.next();
                out.push(b'\n');
            }
            Some('t') => {
                chars.next();
                out.push(b'\t');
            }
            Some('0') => {
                chars.next();
                out.push(0);
            }
            Some('\\') => {
                chars.next();
                out.push(b'\\');
            }
            Some('x') => {
                let mut ahead = chars.clone();
                ahead.next();
                let hex: String = ahead.by_ref().take(2).collect();
                if hex.len() == 2
                    && let Ok(b) = u8::from_str_radix(&hex, 16)
                {
                    out.push(b);
                    chars = ahead;
                } else {
                    out.push(b'\\');
                }
            }
            _ => out.push(b'\\'),
        }
    }
    out
}

/// A short description of what a send cue sends, for the cue list.
pub fn describe(cue: &SendCue) -> String {
    match &cue.target {
        SendTarget::Projector(p) => format!("Projector {}: {}", p.host.trim(), p.action.label()),
        SendTarget::Network(n) => format!(
            "{} {}:{} {}",
            if n.tcp { "TCP" } else { "UDP" },
            n.host.trim(),
            n.port,
            n.payload.trim()
        ),
        SendTarget::Osc(OscOut {
            host,
            port,
            address,
            args,
        }) => {
            let args = args.trim();
            format!(
                "OSC {host}:{port} {address}{}{args}",
                if args.is_empty() { "" } else { " " }
            )
        }
        SendTarget::Midi(m) => {
            let what = match m.kind {
                MidiKind::NoteOn | MidiKind::NoteOff => {
                    format!("{} {} vel {}", m.kind.label(), m.data1, m.data2)
                }
                MidiKind::ControlChange => format!("CC {} = {}", m.data1, m.data2),
                MidiKind::ProgramChange => format!("Program {}", m.data1),
                MidiKind::ShowControl(c) if c.takes_cue() && !m.cue.is_empty() => {
                    format!("MSC {} {}", c.label(), m.cue)
                }
                MidiKind::ShowControl(c) => format!("MSC {}", c.label()),
            };
            let port = m.port.as_deref().unwrap_or("(no output)");
            match m.kind {
                MidiKind::ShowControl(_) => format!("MIDI {port}: {what}"),
                _ => format!("MIDI {port} ch {}: {what}", (m.channel & 0x0F) + 1),
            }
        }
    }
}

/// Names of the MIDI outputs that are connected now.
pub fn midi_outputs() -> Vec<String> {
    let Ok(out) = midir::MidiOutput::new("NewQ out ports") else {
        return Vec::new();
    };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect()
}

/// Where messages go. A real one sends from a worker thread; a recording one
/// (for tests) keeps what it was asked to send.
pub struct Outbox {
    jobs: Option<Sender<Outgoing>>,
    errors: Receiver<String>,
    recorded: Option<Arc<parking_lot::Mutex<Vec<Outgoing>>>>,
}

impl Outbox {
    pub fn new() -> Outbox {
        let (jobs, rx) = unbounded::<Outgoing>();
        let (err_tx, errors) = unbounded();
        let _ = std::thread::Builder::new()
            .name("send-cues".into())
            .spawn(move || {
                let mut sender = SocketSet::default();
                let mut hosts: HashMap<String, Sender<Outgoing>> = HashMap::new();
                for job in rx {
                    // Projectors and TCP hosts can be slow or gone, so each
                    // has a thread of its own: one that doesn't answer holds
                    // up nobody else, and commands to it still go in order.
                    let host_key = match &job {
                        Outgoing::Projector(p) => {
                            Some(format!("pjlink {}:{}", p.host.trim(), p.port))
                        }
                        Outgoing::Tcp { host, port, .. } => Some(format!("tcp {host}:{port}")),
                        _ => None,
                    };
                    if let Some(key) = host_key {
                        let tx = hosts.entry(key.clone()).or_insert_with(|| {
                            let (tx, jobs) = unbounded::<Outgoing>();
                            let errors = err_tx.clone();
                            let _ = std::thread::Builder::new().name(key).spawn(move || {
                                for job in jobs {
                                    if let Err(e) = send_network(&job) {
                                        let _ = errors.send(e);
                                    }
                                }
                            });
                            tx
                        });
                        let _ = tx.send(job);
                    } else if let Err(e) = sender.send(&job) {
                        let _ = err_tx.send(e);
                    }
                }
            });
        Outbox {
            jobs: Some(jobs),
            errors,
            recorded: None,
        }
    }

    /// An outbox that sends nothing and remembers what it was given.
    #[cfg(test)]
    pub fn recording() -> Outbox {
        let (_, errors) = unbounded();
        Outbox {
            jobs: None,
            errors,
            recorded: Some(Arc::new(parking_lot::Mutex::new(Vec::new()))),
        }
    }

    pub fn send(&self, message: Outgoing) {
        if let Some(r) = &self.recorded {
            r.lock().push(message);
        } else if let Some(tx) = &self.jobs {
            let _ = tx.send(message);
        }
    }

    #[cfg(test)]
    pub fn sent(&self) -> Vec<Outgoing> {
        self.recorded
            .as_ref()
            .map(|r| r.lock().clone())
            .unwrap_or_default()
    }

    /// Problems the worker has had since the last call.
    pub fn errors(&self) -> Vec<String> {
        self.errors.try_iter().collect()
    }
}

/// Send what goes over a TCP connection of its own: a projector command or
/// bytes to a host.
fn send_network(job: &Outgoing) -> Result<(), String> {
    match job {
        Outgoing::Projector(p) => crate::projector::send(p),
        Outgoing::Tcp {
            host,
            port,
            payload,
        } => {
            let addr = (host.as_str(), *port)
                .to_socket_addrs()
                .map_err(|e| format!("TCP: can't find {host}: {e}"))?
                .next()
                .ok_or_else(|| format!("TCP: no address for {host}"))?;
            let timeout = Duration::from_secs(4);
            let mut stream = TcpStream::connect_timeout(&addr, timeout)
                .map_err(|e| format!("TCP: can't connect to {host}:{port}: {e}"))?;
            stream.set_write_timeout(Some(timeout)).ok();
            stream
                .write_all(payload)
                .and_then(|()| stream.flush())
                .map_err(|e| format!("TCP to {host}:{port}: {e}"))
        }
        _ => Err("internal: not a network message".into()),
    }
}

/// The sockets and MIDI connections the worker keeps open between sends.
#[derive(Default)]
struct SocketSet {
    udp4: Option<UdpSocket>,
    udp6: Option<UdpSocket>,
    midi: HashMap<String, midir::MidiOutputConnection>,
}

impl SocketSet {
    fn send(&mut self, job: &Outgoing) -> Result<(), String> {
        match job {
            Outgoing::Osc { host, port, packet } => {
                let addr = (host.as_str(), *port)
                    .to_socket_addrs()
                    .map_err(|e| format!("UDP: can't find {host}: {e}"))?
                    .next()
                    .ok_or_else(|| format!("UDP: no address for {host}"))?;
                let socket = if addr.is_ipv4() {
                    &mut self.udp4
                } else {
                    &mut self.udp6
                };
                if socket.is_none() {
                    let bind = if addr.is_ipv4() {
                        "0.0.0.0:0"
                    } else {
                        "[::]:0"
                    };
                    *socket = Some(UdpSocket::bind(bind).map_err(|e| format!("UDP: {e}"))?);
                }
                socket
                    .as_ref()
                    .expect("just opened")
                    .send_to(packet, addr)
                    .map_err(|e| format!("UDP to {host}:{port}: {e}"))?;
                Ok(())
            }
            Outgoing::Projector(_) | Outgoing::Tcp { .. } => send_network(job),
            Outgoing::Midi { port, bytes } => {
                let name = port.as_ref().ok_or("MIDI: no output chosen")?;
                if !self.midi.contains_key(name) {
                    let out = midir::MidiOutput::new("NewQ").map_err(|e| format!("MIDI: {e}"))?;
                    let found = out
                        .ports()
                        .into_iter()
                        .find(|p| out.port_name(p).is_ok_and(|n| &n == name))
                        .ok_or_else(|| format!("MIDI output “{name}” is not connected"))?;
                    let conn = out
                        .connect(&found, "newq-out")
                        .map_err(|e| format!("MIDI: {e}"))?;
                    self.midi.insert(name.clone(), conn);
                }
                let result = self
                    .midi
                    .get_mut(name)
                    .expect("just connected")
                    .send(bytes)
                    .map_err(|e| format!("MIDI to {name}: {e}"));
                if result.is_err() {
                    // Unplugged, perhaps: connect afresh next time.
                    self.midi.remove(name);
                }
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{Command, parse_midi, parse_osc};
    use std::time::Duration;

    #[test]
    fn arguments_are_read_as_numbers_or_text() {
        assert_eq!(
            parse_args(r#"1 -2 0.5 hello "two words" 1e3"#),
            vec![
                OscArg::Int(1),
                OscArg::Int(-2),
                OscArg::Float(0.5),
                OscArg::Text("hello".into()),
                OscArg::Text("two words".into()),
                OscArg::Float(1000.0),
            ]
        );
        assert!(parse_args("").is_empty() && parse_args("   ").is_empty());
        // Words that happen to parse as special floats stay text.
        assert_eq!(
            parse_args("inf nan"),
            vec![OscArg::Text("inf".into()), OscArg::Text("nan".into())]
        );
        // A quote left open takes the rest; a quoted number stays text.
        assert_eq!(parse_args(r#""12""#), vec![OscArg::Text("12".into())]);
        assert_eq!(
            parse_args(r#"a "b c"#),
            vec![OscArg::Text("a".into()), OscArg::Text("b c".into())]
        );
    }

    #[test]
    fn osc_messages_are_laid_out_as_the_spec_says() {
        // No arguments: address, then an empty type tag string.
        assert_eq!(osc_message("/go", &[]), b"/go\0,\0\0\0");
        // An int, a float and a string, each padded to four bytes.
        let m = osc_message(
            "/cue/1/start",
            &[
                OscArg::Int(1),
                OscArg::Float(0.5),
                OscArg::Text("hi".into()),
            ],
        );
        let mut want = b"/cue/1/start\0\0\0\0".to_vec();
        want.extend_from_slice(b",ifs\0\0\0\0");
        want.extend_from_slice(&1i32.to_be_bytes());
        want.extend_from_slice(&0.5f32.to_be_bytes());
        want.extend_from_slice(b"hi\0\0");
        assert_eq!(m, want);
        assert_eq!(m.len() % 4, 0);
    }

    #[test]
    fn what_we_send_is_what_our_own_listeners_understand() {
        for (address, command) in [
            ("/go", Command::Go),
            ("/panic", Command::Panic),
            ("/cue/12/start", Command::Fire("12".into())),
            ("/cue/3.5/stop", Command::Stop("3.5".into())),
        ] {
            let packet = osc_message(address, &parse_args("1 2"));
            assert_eq!(parse_osc(&packet), vec![command], "{address}");
        }
    }

    fn midi(kind: MidiKind, channel: u8, d1: u8, d2: u8, cue: &str) -> MidiOut {
        MidiOut {
            port: Some("Desk".into()),
            kind,
            channel,
            data1: d1,
            data2: d2,
            cue: cue.into(),
        }
    }

    #[test]
    fn midi_messages_have_the_right_bytes() {
        let b = |k, c, d1, d2| midi_bytes(&midi(k, c, d1, d2, ""));
        assert_eq!(b(MidiKind::NoteOn, 0, 60, 100), [0x90, 60, 100]);
        assert_eq!(b(MidiKind::NoteOn, 15, 127, 1), [0x9F, 127, 1]);
        assert_eq!(b(MidiKind::NoteOff, 2, 61, 0), [0x82, 61, 0]);
        assert_eq!(b(MidiKind::ControlChange, 1, 7, 64), [0xB1, 7, 64]);
        assert_eq!(b(MidiKind::ProgramChange, 3, 9, 77), [0xC3, 9]);
        // Out-of-range values are cut to what MIDI can carry.
        assert_eq!(b(MidiKind::NoteOn, 16, 200, 255), [0x90, 72, 127]);
    }

    #[test]
    fn show_control_messages_are_read_back_by_the_msc_parser() {
        let msc = |c, cue: &str| midi_bytes(&midi(MidiKind::ShowControl(c), 0, 0, 0, cue));
        assert_eq!(
            msc(MscCommand::Go, "5"),
            [0xF0, 0x7F, 0x7F, 2, 1, 1, b'5', 0xF7]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::Go, "5"), None),
            vec![Command::GoCue("5".into())]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::Go, ""), None),
            vec![Command::Go]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::Stop, "12.5"), None),
            vec![Command::Stop("12.5".into())]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::Resume, "9"), None),
            vec![Command::Resume]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::Load, "4"), None),
            vec![Command::LoadCue("4".into())]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::Reset, "ignored"), None),
            vec![Command::Reset]
        );
        assert_eq!(
            parse_midi(&msc(MscCommand::AllOff, "ignored"), None),
            vec![Command::Panic]
        );
    }

    #[test]
    fn notes_sent_are_understood_as_notes_received() {
        let bytes = midi_bytes(&midi(MidiKind::NoteOn, 4, 36, 90, ""));
        assert_eq!(parse_midi(&bytes, Some(4)), vec![Command::Note(36)]);
        assert!(parse_midi(&bytes, Some(5)).is_empty());
    }

    #[test]
    fn cues_describe_what_they_send() {
        let mut c = SendCue::default();
        assert_eq!(describe(&c), "OSC 127.0.0.1:53000 /go");
        c.target = SendTarget::Osc(OscOut {
            host: "10.0.0.5".into(),
            port: 9000,
            address: "/cue/2/start".into(),
            args: "1 0.5".into(),
        });
        assert_eq!(describe(&c), "OSC 10.0.0.5:9000 /cue/2/start 1 0.5");
        c.target = SendTarget::Midi(midi(MidiKind::NoteOn, 0, 60, 100, ""));
        assert_eq!(describe(&c), "MIDI Desk ch 1: Note on 60 vel 100");
        c.target = SendTarget::Midi(midi(MidiKind::ShowControl(MscCommand::Go), 0, 0, 0, "7"));
        assert_eq!(describe(&c), "MIDI Desk: MSC GO 7");
        let mut none = midi(MidiKind::ProgramChange, 1, 3, 0, "");
        none.port = None;
        c.target = SendTarget::Midi(none);
        assert_eq!(describe(&c), "MIDI (no output) ch 2: Program 3");
    }

    #[test]
    fn osc_goes_out_over_real_udp() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let port = receiver.local_addr().unwrap().port();
        let outbox = Outbox::new();
        let cue = SendCue {
            target: SendTarget::Osc(OscOut {
                host: " 127.0.0.1 ".into(),
                port,
                address: " /cue/8/start ".into(),
                args: "7".into(),
            }),
        };
        outbox.send(Outgoing::from_cue(&cue));
        let mut buf = [0u8; 256];
        let (n, _) = receiver.recv_from(&mut buf).expect("a packet");
        assert_eq!(&buf[..n], osc_message("/cue/8/start", &[OscArg::Int(7)]));
        assert_eq!(parse_osc(&buf[..n]), vec![Command::Fire("8".into())]);
        assert!(outbox.errors().is_empty());
    }

    #[test]
    fn problems_are_reported_not_fatal() {
        let outbox = Outbox::new();
        outbox.send(Outgoing::Midi {
            port: None,
            bytes: vec![0x90, 60, 100],
        });
        outbox.send(Outgoing::Midi {
            port: Some("No such output".into()),
            bytes: vec![0x90, 60, 100],
        });
        outbox.send(Outgoing::Osc {
            host: "no-such-host.invalid".into(),
            port: 9,
            packet: osc_message("/go", &[]),
        });
        let mut errors = Vec::new();
        for _ in 0..200 {
            errors.extend(outbox.errors());
            if errors.len() >= 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].contains("no output chosen"), "{errors:?}");
        // Where there is no MIDI system at all (a bare server) it says so instead.
        assert!(
            errors[1].contains("No such output") || errors[1].starts_with("MIDI:"),
            "{errors:?}"
        );
        assert!(errors[2].contains("no-such-host"), "{errors:?}");
        // And the worker carries on afterwards.
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        outbox.send(Outgoing::Osc {
            host: "127.0.0.1".into(),
            port: receiver.local_addr().unwrap().port(),
            packet: osc_message("/go", &[]),
        });
        let mut buf = [0u8; 64];
        assert!(receiver.recv_from(&mut buf).is_ok());
    }

    #[test]
    fn escapes_become_bytes() {
        assert_eq!(unescape("PON\\r"), b"PON\r");
        assert_eq!(unescape("\\x02PON\\x03"), [0x02, b'P', b'O', b'N', 0x03]);
        assert_eq!(unescape("a\\nb\\tc\\0"), b"a\nb\tc\0");
        assert_eq!(unescape("one\\\\two"), b"one\\two");
        // Not an escape: left as typed. A short or bad hex pair too.
        assert_eq!(unescape("C:\\dir"), b"C:\\dir");
        assert_eq!(unescape("\\x4"), b"\\x4");
        assert_eq!(unescape("\\xZZ"), b"\\xZZ");
        assert_eq!(unescape("end\\"), b"end\\");
        assert_eq!(unescape("héllo"), "héllo".as_bytes());
    }

    #[test]
    fn network_cues_become_tcp_or_udp() {
        let mut n = NetworkOut {
            host: " 10.0.0.9 ".into(),
            port: 1024,
            tcp: true,
            payload: "\\x02PON\\x03".into(),
        };
        let cue = |n: &NetworkOut| SendCue {
            target: SendTarget::Network(n.clone()),
        };
        assert_eq!(
            Outgoing::from_cue(&cue(&n)),
            Outgoing::Tcp {
                host: "10.0.0.9".into(),
                port: 1024,
                payload: vec![2, b'P', b'O', b'N', 3],
            }
        );
        assert_eq!(describe(&cue(&n)), "TCP 10.0.0.9:1024 \\x02PON\\x03");
        n.tcp = false;
        assert_eq!(
            Outgoing::from_cue(&cue(&n)),
            Outgoing::Osc {
                host: "10.0.0.9".into(),
                port: 1024,
                packet: vec![2, b'P', b'O', b'N', 3],
            }
        );
        assert!(describe(&cue(&n)).starts_with("UDP "));
    }

    #[test]
    fn projector_cues_are_described() {
        let cue = SendCue {
            target: SendTarget::Projector(ProjectorOut::default()),
        };
        assert_eq!(describe(&cue), "Projector 192.168.0.100: Power on");
    }

    #[test]
    fn tcp_bytes_and_projector_commands_go_out_for_real() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let got = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            s.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let outbox = Outbox::new();
        outbox.send(Outgoing::from_cue(&SendCue {
            target: SendTarget::Network(NetworkOut {
                host: "127.0.0.1".into(),
                port,
                tcp: true,
                payload: "hello\\r\\n".into(),
            }),
        }));
        assert_eq!(got.join().unwrap(), b"hello\r\n");

        // A projector that is not there is reported, and does not hold up a
        // message to anywhere else.
        let dead = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        outbox.send(Outgoing::Projector(ProjectorOut {
            port: dead,
            host: "127.0.0.1".into(),
            ..ProjectorOut::default()
        }));
        let mut errors = Vec::new();
        for _ in 0..200 {
            errors.extend(outbox.errors());
            if !errors.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(errors[0].contains("can't connect"), "{errors:?}");
    }
}
