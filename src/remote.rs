//! Remote control: OSC over UDP and MIDI (notes, program changes and MIDI
//! Show Control) turned into commands for the show.
//!
//! The parsing is plain functions on bytes, so it is tested without any
//! network or MIDI hardware. The listeners run on their own threads, queue
//! commands, and wake the event loop to act on them.

use crate::show::RemoteSettings;
use crate::timecode::{self, QuarterFrames, Rate, Timecode};
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// What a remote message asks for. Cues are named by their cue number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Go,
    /// Fade everything out (the Esc key); asked again, stop at once.
    Panic,
    /// Stop everything at once.
    StopAll,
    Pause,
    Resume,
    PlayheadNext,
    PlayheadPrevious,
    /// Put the playhead on a cue, then GO.
    GoCue(String),
    /// Put the playhead on a cue.
    Select(String),
    /// Fire a cue without moving the playhead.
    Fire(String),
    /// A MIDI note: fires the cue given that note, or else the cue with that number.
    Note(u8),
    Stop(String),
    FadeOut(String),
    /// Arm or disarm a cue.
    Arm(String),
    Disarm(String),
    /// MIDI Time Code reached this position (every two frames while it runs).
    Timecode(Timecode, Rate),
    /// The timecode source located here (a full frame message).
    TimecodeLocate(Timecode, Rate),
}

// ----- OSC ------------------------------------------------------------------

/// Read a NUL-terminated OSC string padded to a multiple of four bytes.
fn osc_string(data: &[u8]) -> Option<(&str, &[u8])> {
    let end = data.iter().position(|&b| b == 0)?;
    let s = std::str::from_utf8(&data[..end]).ok()?;
    let used = (end + 4) & !3;
    Some((s, data.get(used..).unwrap_or(&[])))
}

/// Commands in an OSC packet: one message, or a bundle of them. Arguments
/// are ignored; the address says everything.
pub fn parse_osc(packet: &[u8]) -> Vec<Command> {
    let mut out = Vec::new();
    parse_osc_into(packet, &mut out, 0);
    out
}

fn parse_osc_into(packet: &[u8], out: &mut Vec<Command>, depth: u32) {
    if packet.starts_with(b"#bundle\0") {
        if depth > 4 {
            return;
        }
        // 8 bytes of time tag, then size-prefixed elements.
        let mut rest = packet.get(16..).unwrap_or(&[]);
        while rest.len() >= 4 {
            let len = i32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
            let Ok(len) = usize::try_from(len) else {
                return;
            };
            let Some(element) = rest.get(4..4 + len) else {
                return;
            };
            parse_osc_into(element, out, depth + 1);
            rest = &rest[4 + len..];
        }
        return;
    }
    if let Some((address, _)) = osc_string(packet)
        && let Some(cmd) = osc_address(address)
    {
        out.push(cmd);
    }
}

/// The command for an OSC address such as `/go` or `/cue/3.5/start`.
pub fn osc_address(address: &str) -> Option<Command> {
    let parts: Vec<&str> = address.split('/').skip(1).collect();
    let lower: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    let words: Vec<&str> = lower.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["go"] => Some(Command::Go),
        ["panic"] => Some(Command::Panic),
        ["stop"] => Some(Command::StopAll),
        ["pause"] => Some(Command::Pause),
        ["resume"] => Some(Command::Resume),
        ["playhead", "next"] => Some(Command::PlayheadNext),
        ["playhead", "previous"] => Some(Command::PlayheadPrevious),
        // Cue numbers are matched as written, not lower-cased.
        ["cue", _, action] if !parts[1].is_empty() => {
            let n = parts[1].to_string();
            match *action {
                "start" => Some(Command::Fire(n)),
                "go" => Some(Command::GoCue(n)),
                "select" | "load" => Some(Command::Select(n)),
                "stop" => Some(Command::Stop(n)),
                "fade" => Some(Command::FadeOut(n)),
                "arm" => Some(Command::Arm(n)),
                "disarm" => Some(Command::Disarm(n)),
                _ => None,
            }
        }
        _ => None,
    }
}

// ----- MIDI -----------------------------------------------------------------

/// A cue number in MIDI Show Control data: ASCII digits and dots up to the
/// first NUL (a cue list and path may follow).
fn msc_cue(data: &[u8]) -> Option<String> {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let s: String = data[..end]
        .iter()
        .filter(|b| b.is_ascii_graphic())
        .map(|&b| b as char)
        .collect();
    (!s.is_empty()).then_some(s)
}

/// Commands for one MIDI message. `channel` limits channel messages to one
/// channel (0 to 15); None accepts any. A note-on fires the cue given that
/// note (or else the cue with that number), a program change puts the playhead on it, and MIDI Show Control
/// (GO, STOP, RESUME, LOAD and ALL_OFF) drives the transport.
pub fn parse_midi(msg: &[u8], channel: Option<u8>) -> Vec<Command> {
    let Some(&status) = msg.first() else {
        return Vec::new();
    };
    if status == 0xF0 {
        if let Some((tc, rate)) = timecode::parse_full_frame(msg) {
            return vec![Command::TimecodeLocate(tc, rate)];
        }
        // F0 7F <device> 02 <format> <command> <data...> F7
        if msg.len() < 7 || msg[1] != 0x7F || msg[3] != 0x02 {
            return Vec::new();
        }
        let end = if msg.last() == Some(&0xF7) {
            msg.len() - 1
        } else {
            msg.len()
        };
        let cue = msc_cue(&msg[6.min(end)..end]);
        return match (msg[5], cue) {
            (0x01 | 0x04, Some(n)) => vec![Command::GoCue(n)],
            (0x01 | 0x04, None) => vec![Command::Go],
            (0x02, Some(n)) => vec![Command::Stop(n)],
            (0x02, None) => vec![Command::StopAll],
            (0x03, _) => vec![Command::Resume],
            (0x05, Some(n)) => vec![Command::Select(n)],
            (0x08, _) => vec![Command::Panic],
            _ => Vec::new(),
        };
    }
    if !(0x80..0xF0).contains(&status) {
        return Vec::new();
    }
    if channel.is_some_and(|c| c != status & 0x0F) {
        return Vec::new();
    }
    match (status & 0xF0, msg.get(1), msg.get(2)) {
        (0x90, Some(&note), Some(&vel)) if vel > 0 => vec![Command::Note(note)],
        (0xC0, Some(&program), _) => vec![Command::Select(program.to_string())],
        _ => Vec::new(),
    }
}

/// Commands for one message from the MIDI input. Timecode is only followed
/// when the show asks for it (`mtc`): otherwise it is a flood of messages with
/// no use. Quarter frames (`F1 dd`) are assembled in `quarter_frames`.
pub fn midi_commands(
    msg: &[u8],
    channel: Option<u8>,
    mtc: bool,
    quarter_frames: &mut QuarterFrames,
) -> Vec<Command> {
    if msg.first() == Some(&0xF1) {
        return match msg.get(1).and_then(|&b| quarter_frames.push(b)) {
            Some((tc, rate)) if mtc => vec![Command::Timecode(tc, rate)],
            _ => Vec::new(),
        };
    }
    parse_midi(msg, channel)
        .into_iter()
        .filter(|c| mtc || !matches!(c, Command::TimecodeLocate(..)))
        .collect()
}

// ----- Listeners ------------------------------------------------------------

/// Receives OSC packets on a UDP port.
struct OscListener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    port: u16,
}

impl OscListener {
    fn start(
        port: u16,
        tx: Sender<Command>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
        // Wake now and then to notice that we were asked to stop.
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("osc-listener".into())
            .spawn(move || {
                let mut buf = vec![0u8; 65536];
                while !stop2.load(Ordering::Relaxed) {
                    let Ok((n, _)) = socket.recv_from(&mut buf) else {
                        continue;
                    };
                    let commands = parse_osc(&buf[..n]);
                    if commands.is_empty() {
                        continue;
                    }
                    for c in commands {
                        let _ = tx.send(c);
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(OscListener {
            stop,
            thread: Some(thread),
            port,
        })
    }
}

impl Drop for OscListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What the settings panel shows about the listeners.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteStatus {
    /// The UDP port OSC is listening on.
    pub osc_port: Option<u16>,
    pub osc_error: Option<String>,
    /// The MIDI input that is connected.
    pub midi_port: Option<String>,
    pub midi_error: Option<String>,
}

/// Runs the listeners the show's settings ask for.
pub struct Remote {
    tx: Sender<Command>,
    rx: Receiver<Command>,
    wake: Arc<dyn Fn() + Send + Sync>,
    osc: Option<OscListener>,
    midi: Option<midir::MidiInputConnection<()>>,
    /// The settings the listeners were last started with.
    started: Option<RemoteSettings>,
    pub status: RemoteStatus,
}

impl Remote {
    /// `wake` is called (from a listener thread) when commands are waiting.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Remote {
        let (tx, rx) = unbounded();
        Remote {
            tx,
            rx,
            wake: Arc::new(wake),
            osc: None,
            midi: None,
            started: None,
            status: RemoteStatus::default(),
        }
    }

    /// Bring the listeners in line with `wanted`, restarting only what changed.
    pub fn sync(&mut self, wanted: &RemoteSettings) {
        let old = self.started.clone().unwrap_or_default();
        let first = self.started.is_none();
        if first || old.osc != wanted.osc || old.osc_port != wanted.osc_port {
            self.osc = None;
            self.status.osc_port = None;
            self.status.osc_error = None;
            if wanted.osc {
                match OscListener::start(wanted.osc_port, self.tx.clone(), self.wake.clone()) {
                    Ok(l) => {
                        self.status.osc_port = Some(l.port);
                        self.osc = Some(l);
                    }
                    Err(e) => self.status.osc_error = Some(e),
                }
            }
        }
        if first
            || old.midi != wanted.midi
            || old.midi_port != wanted.midi_port
            || old.mtc != wanted.mtc
        {
            self.midi = None;
            self.status.midi_port = None;
            self.status.midi_error = None;
            if wanted.midi {
                match self.connect_midi(wanted) {
                    Ok(c) => {
                        self.status.midi_port = wanted.midi_port.clone();
                        self.midi = Some(c);
                    }
                    Err(e) => self.status.midi_error = Some(e),
                }
            }
        }
        // The channel filter applies in the callback, which reads it at connect
        // time, so a changed channel needs a reconnect too.
        if !first && old.midi_channel != wanted.midi_channel && wanted.midi && self.midi.is_some() {
            self.midi = None;
            match self.connect_midi(wanted) {
                Ok(c) => self.midi = Some(c),
                Err(e) => {
                    self.status.midi_port = None;
                    self.status.midi_error = Some(e);
                }
            }
        }
        self.started = Some(wanted.clone());
    }

    fn connect_midi(
        &self,
        wanted: &RemoteSettings,
    ) -> Result<midir::MidiInputConnection<()>, String> {
        let mut input = midir::MidiInput::new("NewQ").map_err(|e| e.to_string())?;
        // Sysex is how MIDI Show Control arrives.
        input.ignore(midir::Ignore::None);
        let name = wanted.midi_port.clone().ok_or("Choose a MIDI input")?;
        let port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).is_ok_and(|n| n == name))
            .ok_or_else(|| format!("MIDI input “{name}” is not connected"))?;
        let (tx, wake, channel) = (self.tx.clone(), self.wake.clone(), wanted.midi_channel);
        let mtc = wanted.mtc;
        let mut quarter_frames = QuarterFrames::default();
        input
            .connect(
                &port,
                "newq-in",
                move |_, msg, _| {
                    let commands = midi_commands(msg, channel, mtc, &mut quarter_frames);
                    if !commands.is_empty() {
                        for c in commands {
                            let _ = tx.send(c);
                        }
                        wake();
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())
    }

    /// Commands that have arrived since the last call.
    pub fn drain(&self) -> Vec<Command> {
        self.rx.try_iter().collect()
    }
}

/// Names of the MIDI inputs that are connected now.
pub fn midi_inputs() -> Vec<String> {
    let Ok(input) = midir::MidiInput::new("NewQ ports") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OSC message with the given address and no arguments.
    fn osc(address: &str) -> Vec<u8> {
        let mut v = address.as_bytes().to_vec();
        v.push(0);
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
        v.extend_from_slice(b",\0\0\0");
        v
    }

    fn bundle(elements: &[Vec<u8>]) -> Vec<u8> {
        let mut v = b"#bundle\0".to_vec();
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        for e in elements {
            v.extend_from_slice(&(e.len() as i32).to_be_bytes());
            v.extend_from_slice(e);
        }
        v
    }

    #[test]
    fn osc_addresses_map_to_commands() {
        let cases = [
            ("/go", Command::Go),
            ("/panic", Command::Panic),
            ("/stop", Command::StopAll),
            ("/pause", Command::Pause),
            ("/resume", Command::Resume),
            ("/playhead/next", Command::PlayheadNext),
            ("/playhead/previous", Command::PlayheadPrevious),
            ("/cue/12/start", Command::Fire("12".into())),
            ("/cue/3.5/go", Command::GoCue("3.5".into())),
            ("/cue/3.5/select", Command::Select("3.5".into())),
            ("/cue/3.5/load", Command::Select("3.5".into())),
            ("/cue/7/stop", Command::Stop("7".into())),
            ("/cue/7/fade", Command::FadeOut("7".into())),
            ("/cue/7/arm", Command::Arm("7".into())),
            ("/cue/7/disarm", Command::Disarm("7".into())),
            ("/GO", Command::Go),
            ("/cue/Q1a/START", Command::Fire("Q1a".into())),
        ];
        for (addr, want) in cases {
            assert_eq!(parse_osc(&osc(addr)), vec![want], "{addr}");
        }
    }

    #[test]
    fn unknown_or_malformed_osc_is_ignored() {
        for addr in [
            "/nothing",
            "/cue//start",
            "/cue/1",
            "/cue/1/dance",
            "go",
            "/go/now",
        ] {
            assert!(parse_osc(&osc(addr)).is_empty(), "{addr}");
        }
        assert!(parse_osc(&[]).is_empty());
        assert!(parse_osc(b"/go").is_empty(), "missing terminator");
        assert!(parse_osc(&[0xff, 0xfe, 0, 0]).is_empty());
        // A bundle that lies about an element's size.
        let mut b = bundle(&[osc("/go")]);
        b[16..20].copy_from_slice(&1000i32.to_be_bytes());
        assert!(parse_osc(&b).is_empty());
        let mut neg = bundle(&[osc("/go")]);
        neg[16..20].copy_from_slice(&(-4i32).to_be_bytes());
        assert!(parse_osc(&neg).is_empty());
    }

    #[test]
    fn osc_arguments_are_ignored_and_bundles_unpacked() {
        let mut with_args = osc("/go");
        with_args.truncate(with_args.len() - 4);
        with_args.extend_from_slice(b",if\0");
        with_args.extend_from_slice(&[0, 0, 0, 1, 0x3f, 0x80, 0, 0]);
        assert_eq!(parse_osc(&with_args), vec![Command::Go]);

        let b = bundle(&[osc("/cue/1/start"), osc("/go"), osc("/bogus")]);
        assert_eq!(parse_osc(&b), vec![Command::Fire("1".into()), Command::Go]);
        // Bundles inside bundles.
        let nested = bundle(&[bundle(&[osc("/panic")]), osc("/pause")]);
        assert_eq!(parse_osc(&nested), vec![Command::Panic, Command::Pause]);
    }

    /// A MIDI Show Control message: F0 7F device 02 format command data F7.
    fn msc(command: u8, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0xF0, 0x7F, 0x00, 0x02, 0x01, command];
        v.extend_from_slice(data);
        v.push(0xF7);
        v
    }

    #[test]
    fn midi_show_control_drives_the_transport() {
        assert_eq!(parse_midi(&msc(0x01, b""), None), vec![Command::Go]);
        assert_eq!(
            parse_midi(&msc(0x01, b"3.5"), None),
            vec![Command::GoCue("3.5".into())]
        );
        // A cue list after the cue number is ignored.
        assert_eq!(
            parse_midi(&msc(0x01, b"12\x001"), None),
            vec![Command::GoCue("12".into())]
        );
        assert_eq!(
            parse_midi(&msc(0x04, b"4"), None),
            vec![Command::GoCue("4".into())]
        );
        assert_eq!(parse_midi(&msc(0x02, b""), None), vec![Command::StopAll]);
        assert_eq!(
            parse_midi(&msc(0x02, b"2"), None),
            vec![Command::Stop("2".into())]
        );
        assert_eq!(parse_midi(&msc(0x03, b""), None), vec![Command::Resume]);
        assert_eq!(
            parse_midi(&msc(0x05, b"9"), None),
            vec![Command::Select("9".into())]
        );
        assert_eq!(parse_midi(&msc(0x08, b""), None), vec![Command::Panic]);
        // Commands NewQ doesn't act on, and other sysex.
        assert!(parse_midi(&msc(0x0A, b""), None).is_empty());
        assert!(parse_midi(&[0xF0, 0x41, 0x10, 0xF7], None).is_empty());
        assert!(parse_midi(&[0xF0, 0x7F, 0x00, 0x04, 0x01, 0x01, 0xF7], None).is_empty());
        // The device id and channel filter don't apply to show control.
        assert_eq!(parse_midi(&msc(0x01, b""), Some(5)), vec![Command::Go]);
    }

    #[test]
    fn midi_notes_fire_cues_and_programs_select_them() {
        assert_eq!(parse_midi(&[0x90, 60, 100], None), vec![Command::Note(60)]);
        // A note-on with no velocity is a note-off.
        assert!(parse_midi(&[0x90, 60, 0], None).is_empty());
        assert!(parse_midi(&[0x80, 60, 64], None).is_empty());
        assert_eq!(
            parse_midi(&[0xC3, 7], None),
            vec![Command::Select("7".into())]
        );
        assert!(parse_midi(&[0xB0, 1, 2], None).is_empty(), "control change");
    }

    #[test]
    fn midi_channel_filter() {
        // Channel 3 (index 2).
        let on = [0x92, 10, 90];
        assert_eq!(parse_midi(&on, Some(2)), vec![Command::Note(10)]);
        assert_eq!(parse_midi(&on, None), vec![Command::Note(10)]);
        assert!(parse_midi(&on, Some(0)).is_empty());
        assert!(parse_midi(&[0xC2, 1], Some(3)).is_empty());
    }

    #[test]
    fn truncated_and_system_midi_is_ignored() {
        assert!(parse_midi(&[], None).is_empty());
        assert!(parse_midi(&[0x90], None).is_empty());
        assert!(parse_midi(&[0x90, 60], None).is_empty());
        assert!(parse_midi(&[0xF8], None).is_empty(), "clock");
        assert!(
            parse_midi(&[0x40, 1, 2], None).is_empty(),
            "data byte first"
        );
        assert!(parse_midi(&[0xF0, 0x7F], None).is_empty());
    }

    #[test]
    fn osc_arrives_over_udp_and_wakes_the_loop() {
        let woken = Arc::new(AtomicBool::new(false));
        let w = woken.clone();
        let mut remote = Remote::new(move || w.store(true, Ordering::SeqCst));
        remote.sync(&RemoteSettings {
            osc: true,
            osc_port: 0, // any free port
            ..Default::default()
        });
        let port = remote.status.osc_port.expect("listening");
        assert!(port > 0);
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(&osc("/go"), ("127.0.0.1", port)).unwrap();
        sender.send_to(b"junk", ("127.0.0.1", port)).unwrap();
        sender
            .send_to(&osc("/cue/5/start"), ("127.0.0.1", port))
            .unwrap();
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(remote.drain());
            if got.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, vec![Command::Go, Command::Fire("5".into())]);
        assert!(woken.load(Ordering::SeqCst));
    }

    #[test]
    fn settings_start_and_stop_the_listener() {
        let mut remote = Remote::new(|| {});
        let on = RemoteSettings {
            osc: true,
            osc_port: 0,
            ..Default::default()
        };
        remote.sync(&on);
        assert!(remote.status.osc_port.is_some() && remote.status.osc_error.is_none());
        remote.sync(&RemoteSettings::default());
        assert!(remote.status.osc_port.is_none());
        assert!(remote.osc.is_none());
        // A port already in use is reported, not fatal.
        let taken = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        remote.sync(&RemoteSettings {
            osc: true,
            osc_port: port,
            ..Default::default()
        });
        assert!(remote.status.osc_error.is_some() && remote.status.osc_port.is_none());
    }

    #[test]
    fn a_missing_midi_input_is_reported() {
        let mut remote = Remote::new(|| {});
        remote.sync(&RemoteSettings {
            midi: true,
            midi_port: Some("No such device".into()),
            ..Default::default()
        });
        assert!(remote.status.midi_error.is_some());
        assert!(remote.midi.is_none());
        remote.sync(&RemoteSettings {
            midi: true,
            midi_port: None,
            ..Default::default()
        });
        assert!(remote.status.midi_error.is_some(), "no input chosen");
    }

    #[test]
    fn timecode_is_only_followed_when_asked_for() {
        // Eight quarter frames saying 00:00:01:00 at 25 fps, then the full-frame form.
        let run = [0x00, 0x10, 0x21, 0x30, 0x40, 0x50, 0x60, 0x72];
        let full = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20, 0, 1, 0, 0xF7];
        for mtc in [false, true] {
            let mut q = QuarterFrames::default();
            let mut got = Vec::new();
            for b in run {
                got.extend(midi_commands(&[0xF1, b], None, mtc, &mut q));
            }
            got.extend(midi_commands(&full, None, mtc, &mut q));
            if mtc {
                assert_eq!(
                    got,
                    vec![
                        Command::Timecode(Timecode::new(0, 0, 1, 2), Rate::Fps25),
                        Command::TimecodeLocate(Timecode::new(0, 0, 1, 0), Rate::Fps25),
                    ]
                );
            } else {
                assert!(got.is_empty(), "{got:?}");
            }
        }
        // Everything else still passes through either way, and F1 itself isn't
        // a channel message.
        let mut q = QuarterFrames::default();
        assert_eq!(
            midi_commands(&[0x90, 60, 100], None, false, &mut q),
            vec![Command::Note(60)]
        );
        assert!(parse_midi(&[0xF1, 0x00], None).is_empty());
    }
}
