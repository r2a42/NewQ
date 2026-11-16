//! What computers of a show network say to each other over TCP, and the beacon
//! they announce themselves with over UDP.
//!
//! A TCP frame is a 4-byte big-endian length and that many bytes of JSON, one
//! `Msg`. A frame over `MAX_FRAME` is refused (the session is closed), as is
//! one that isn't JSON. `PROTOCOL` changes only when two NewQs could no longer
//! understand each other; a session's first frame is a `Hello`, read leniently
//! so that a NewQ speaking another protocol is told apart and named. Within a
//! protocol, later stages add `Msg` variants (the clock, commands, show sync,
//! media); a NewQ that doesn't know one reads it as `Msg::Unknown` and lets it
//! pass.

use super::node::{Key, NetworkId, NodeId, Refusal, Role};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// The protocol this NewQ speaks.
pub const PROTOCOL: u32 = 1;

/// This NewQ's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The largest frame accepted: room for a large show (later stages send it
/// whole), not for a runaway length.
pub const MAX_FRAME: usize = 64 << 20;

/// The first frame each side sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub proto: u32,
    pub version: String,
    pub id: NodeId,
    pub name: String,
    pub role: Role,
    #[serde(default)]
    pub network: Option<NetworkId>,
    #[serde(default)]
    pub term: u64,
    /// What the other side's passcode proof is made over (see `Key::proof`).
    pub nonce: u64,
}

/// What one computer tells another about itself, every second and on a change
/// (an adopted computer to its primary).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusReport {
    pub name: String,
    pub role: Role,
    pub version: String,
    pub network: Option<NetworkId>,
    pub primary: Option<NodeId>,
    pub term: u64,
    /// Its monitors, in the system's order (the index a display keeps).
    pub monitors: Vec<MonitorReport>,
    /// Its audio output devices, named as its audio settings list them.
    pub audio_devices: Vec<String>,
    /// Bytes free on the disk its log folder is on.
    pub disk_free: Option<u64>,
    pub link: Link,
    /// Filled by later stages: the shared clock's error, the show version it
    /// has, and how much of the media it has (0 to 1).
    pub clock_error_ms: Option<f64>,
    pub show_version: Option<u64>,
    pub media_progress: Option<f32>,
}

/// A monitor as a computer reports it: what a display of the show keeps to
/// find it again (as `show::MonitorKey` has it), and its size.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorReport {
    /// Its make and model, else the name the system gives it.
    pub identity: String,
    /// The name the system gives it.
    pub raw: String,
    pub hardware: Option<String>,
    pub size: (u32, u32),
    pub refresh_hz: Option<f32>,
}

impl MonitorReport {
    pub fn key(&self) -> crate::show::MonitorKey {
        crate::show::MonitorKey {
            identity: self.identity.clone(),
            raw: self.raw.clone(),
            hardware: self.hardware.clone(),
        }
    }
}

/// How a computer reaches the network, where that can be told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Link {
    Wired,
    WiFi,
    #[default]
    Unknown,
}

impl Link {
    pub fn label(self) -> &'static str {
        match self {
            Link::Wired => "Wired",
            Link::WiFi => "Wi-Fi",
            Link::Unknown => "",
        }
    }

    /// A guess from the adapter's name ("Wi-Fi", "wlan0", "Ethernet",
    /// "enp3s0"); unknown when the name doesn't say (a Mac's "en0").
    pub fn from_adapter_name(name: &str) -> Link {
        let n = name.to_ascii_lowercase();
        if n.contains("wi-fi")
            || n.contains("wifi")
            || n.contains("wlan")
            || n.contains("wireless")
            || n.starts_with("wl")
            || n.contains("802.11")
        {
            Link::WiFi
        } else if n.contains("ethernet")
            || n.starts_with("eth")
            || n.starts_with("enp")
            || n.starts_with("eno")
            || n.starts_with("ens")
            || n.starts_with("enx")
            || n.contains("thunderbolt")
        {
            Link::Wired
        } else {
            Link::Unknown
        }
    }
}

/// Everything sent over a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    Hello(Hello),
    /// The sender's passcode proof over the other's nonce (None: it has no key).
    Prove {
        #[serde(default)]
        mac: Option<String>,
    },
    /// The sender takes orders from the other (a primary) or is open to being
    /// adopted by it.
    Welcome,
    /// The sender won't go on; the session closes.
    Refused {
        why: Refusal,
        text: String,
    },
    /// Primary to a computer: join this network (again, to change the passcode).
    Adopt {
        network: NetworkId,
        term: u64,
        #[serde(default)]
        key: Option<Key>,
        role: Role,
        primary_name: String,
    },
    /// Primary to a computer: leave the network, back to waiting.
    Release,
    SetRole {
        role: Role,
    },
    Rename {
        name: String,
    },
    /// Show the computer's name big on it for a few seconds.
    Identify {
        seconds: f32,
    },
    Status(Box<StatusReport>),
    Ping {
        n: u64,
    },
    Pong {
        n: u64,
    },
    /// A message a later NewQ of the same protocol sent, unknown here.
    #[serde(other)]
    Unknown,
}

/// Why a frame couldn't be read.
#[derive(Debug, PartialEq)]
pub enum FrameError {
    /// The connection ended or failed.
    Closed(String),
    /// A length over `MAX_FRAME`.
    TooBig(usize),
    /// Not a message.
    Garbage(String),
    /// A first frame from a NewQ of another protocol: its name and version.
    Mismatch {
        name: String,
        version: String,
        proto: u32,
    },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Closed(e) => write!(f, "the connection ended ({e})"),
            FrameError::TooBig(n) => write!(f, "a message of {n} bytes, over the limit"),
            FrameError::Garbage(e) => write!(f, "a message that couldn't be read ({e})"),
            FrameError::Mismatch { name, version, .. } => {
                f.write_str(&mismatch_sentence(name, version, VERSION))
            }
        }
    }
}

/// The frame for `msg`.
pub fn encode(msg: &Msg) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_default();
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
pub fn write_frame(w: &mut impl std::io::Write, msg: &Msg) -> std::io::Result<()> {
    w.write_all(&encode(msg))?;
    w.flush()
}

/// The next frame's bytes (blocking).
pub fn read_frame(r: &mut impl Read) -> Result<Vec<u8>, FrameError> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head)
        .map_err(|e| FrameError::Closed(e.to_string()))?;
    let n = u32::from_be_bytes(head) as usize;
    if n > MAX_FRAME {
        return Err(FrameError::TooBig(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)
        .map_err(|e| FrameError::Closed(e.to_string()))?;
    Ok(body)
}

pub fn decode(body: &[u8]) -> Result<Msg, FrameError> {
    serde_json::from_slice(body).map_err(|e| FrameError::Garbage(e.to_string()))
}

/// A session's first frame: a `Hello` of this protocol, or what a NewQ of
/// another protocol is called and which version it is.
pub fn decode_hello(body: &[u8]) -> Result<Hello, FrameError> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| FrameError::Garbage(e.to_string()))?;
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let proto = v.get("proto").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
    if field("t") != "hello" {
        return Err(FrameError::Garbage(
            "the first message wasn't a hello".into(),
        ));
    }
    if proto != PROTOCOL {
        return Err(FrameError::Mismatch {
            name: field("name"),
            version: field("version"),
            proto,
        });
    }
    match serde_json::from_value(v) {
        Ok(Msg::Hello(h)) => Ok(h),
        Ok(_) => Err(FrameError::Garbage("not a hello".into())),
        Err(e) => Err(FrameError::Garbage(e.to_string())),
    }
}

/// "Booth-PC runs NewQ 0.24, this is 0.23: update one of them." Versions are
/// shortened to their first two numbers unless that makes them the same.
pub fn mismatch_sentence(their_name: &str, theirs: &str, ours: &str) -> String {
    let short = |v: &str| v.split('.').take(2).collect::<Vec<_>>().join(".");
    let (a, b) = if short(theirs) == short(ours) {
        (theirs.to_string(), ours.to_string())
    } else {
        (short(theirs), short(ours))
    };
    let name = if their_name.trim().is_empty() {
        "The other computer"
    } else {
        their_name
    };
    let theirs = if a.is_empty() {
        "an older NewQ".into()
    } else {
        format!("NewQ {a}")
    };
    format!("{name} runs {theirs}, this is {b}: update one of them.")
}

/// What a computer announces each second (D3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beacon {
    pub proto: u32,
    pub version: String,
    pub id: NodeId,
    pub name: String,
    pub role: Role,
    pub network: Option<NetworkId>,
    /// The port its sessions are accepted on.
    pub tcp_port: u16,
    pub show_version: Option<u64>,
    pub term: u64,
    /// Switched on and open to being adopted or run.
    pub on: bool,
}

/// What starts every beacon datagram.
const MAGIC: &[u8] = b"NEWQ-NET1";

/// The largest beacon read.
pub const MAX_BEACON: usize = 4096;

pub fn encode_beacon(b: &Beacon) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend(serde_json::to_vec(b).unwrap_or_default());
    out
}

/// A beacon, or None for anything else that arrived on the port. A beacon of
/// another protocol still reads (its name and version are what the page says).
pub fn decode_beacon(data: &[u8]) -> Option<Beacon> {
    let body = data.strip_prefix(MAGIC)?;
    let b: Beacon = serde_json::from_slice(body).ok()?;
    (b.id != NodeId(0)).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            proto: PROTOCOL,
            version: VERSION.into(),
            id: NodeId(5),
            name: "Booth".into(),
            role: Role::Primary,
            network: Some(NetworkId(9)),
            term: 2,
            nonce: 77,
        }
    }

    #[test]
    fn frames_go_and_come_back() {
        let msgs = vec![
            Msg::Hello(hello()),
            Msg::Prove {
                mac: Some("ab".into()),
            },
            Msg::Prove { mac: None },
            Msg::Welcome,
            Msg::Refused {
                why: Refusal::OldTerm { seen: 3 },
                text: "no".into(),
            },
            Msg::Adopt {
                network: NetworkId(9),
                term: 1,
                key: Some(Key::from_passcode("p")),
                role: Role::Display,
                primary_name: "Booth".into(),
            },
            Msg::Release,
            Msg::SetRole { role: Role::Backup },
            Msg::Rename {
                name: "Left".into(),
            },
            Msg::Identify { seconds: 5.0 },
            Msg::Status(Box::new(StatusReport {
                name: "Left".into(),
                monitors: vec![MonitorReport {
                    identity: "DELL".into(),
                    raw: "DP-1".into(),
                    hardware: Some("DEL:1".into()),
                    size: (1920, 1080),
                    refresh_hz: Some(60.0),
                }],
                audio_devices: vec!["Speakers".into()],
                disk_free: Some(5),
                link: Link::Wired,
                ..Default::default()
            })),
            Msg::Ping { n: 1 },
            Msg::Pong { n: 1 },
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            write_frame(&mut wire, m).unwrap();
        }
        let mut r = &wire[..];
        assert_eq!(decode_hello(&read_frame(&mut r).unwrap()).unwrap(), hello());
        for m in &msgs[1..] {
            assert_eq!(&decode(&read_frame(&mut r).unwrap()).unwrap(), m);
        }
        assert!(matches!(read_frame(&mut r), Err(FrameError::Closed(_))));
    }

    #[test]
    fn an_oversized_length_or_garbage_is_refused() {
        let mut wire = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        wire.extend([0u8; 16]);
        assert_eq!(
            read_frame(&mut &wire[..]),
            Err(FrameError::TooBig(MAX_FRAME + 1))
        );
        assert!(matches!(decode(b"\x00\xffzz"), Err(FrameError::Garbage(_))));
        assert!(matches!(decode(b"{\"x\":1}"), Err(FrameError::Garbage(_))));
        assert!(matches!(decode_hello(b"nope"), Err(FrameError::Garbage(_))));
        assert!(matches!(
            decode_hello(&serde_json::to_vec(&Msg::Welcome).unwrap()),
            Err(FrameError::Garbage(_))
        ));
        // A frame cut short is the connection ending.
        let mut cut = encode(&Msg::Welcome);
        cut.truncate(cut.len() - 2);
        assert!(matches!(
            read_frame(&mut &cut[..]),
            Err(FrameError::Closed(_))
        ));
    }

    #[test]
    fn a_message_a_later_newq_adds_is_let_pass() {
        assert_eq!(
            decode(br#"{"t":"clock_sync","at":5}"#).unwrap(),
            Msg::Unknown
        );
        // And fields it adds to a known one are ignored.
        assert_eq!(
            decode(br#"{"t":"ping","n":3,"extra":true}"#).unwrap(),
            Msg::Ping { n: 3 }
        );
    }

    #[test]
    fn another_protocol_is_named_with_its_version() {
        let mut v = serde_json::to_value(Msg::Hello(hello())).unwrap();
        v["proto"] = 2.into();
        v["version"] = "0.24.0".into();
        v["name"] = "Booth-PC".into();
        v["nonce"] = "something else entirely".into();
        let e = decode_hello(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert_eq!(
            e,
            FrameError::Mismatch {
                name: "Booth-PC".into(),
                version: "0.24.0".into(),
                proto: 2
            }
        );
        assert_eq!(
            mismatch_sentence("Booth-PC", "0.24.0", "0.23.1"),
            "Booth-PC runs NewQ 0.24, this is 0.23: update one of them."
        );
        assert_eq!(
            mismatch_sentence("A", "0.23.2", "0.23.1"),
            "A runs NewQ 0.23.2, this is 0.23.1: update one of them."
        );
    }

    #[test]
    fn beacons_go_and_come_back_and_strangers_are_ignored() {
        let b = Beacon {
            proto: PROTOCOL,
            version: VERSION.into(),
            id: NodeId(3),
            name: "Left".into(),
            role: Role::Display,
            network: Some(NetworkId(4)),
            tcp_port: 47820,
            show_version: None,
            term: 1,
            on: true,
        };
        let data = encode_beacon(&b);
        assert!(data.len() < MAX_BEACON);
        assert_eq!(decode_beacon(&data), Some(b));
        assert_eq!(decode_beacon(b"hello"), None);
        assert_eq!(decode_beacon(b"NEWQ-NET1{oops"), None);
        assert_eq!(decode_beacon(b"NEWQ-NET1{}"), None, "no id");
    }

    #[test]
    fn the_link_is_guessed_from_the_adapter_name() {
        assert_eq!(Link::from_adapter_name("Wi-Fi"), Link::WiFi);
        assert_eq!(Link::from_adapter_name("wlp2s0"), Link::WiFi);
        assert_eq!(Link::from_adapter_name("Ethernet 2"), Link::Wired);
        assert_eq!(Link::from_adapter_name("enp3s0"), Link::Wired);
        assert_eq!(Link::from_adapter_name("en0"), Link::Unknown);
    }
}
