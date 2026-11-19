//! Remote control: OSC over UDP and MIDI (notes, program changes and MIDI
//! Show Control) turned into commands for the show.
//!
//! The parsing is plain functions on bytes, so it is tested without any
//! network or MIDI hardware. The listeners run on their own threads, queue
//! commands, and wake the event loop to act on them.

use crate::engine::Engine;
use crate::show::{CueId, PauseFreeze, RemoteSettings, Show, TIMELINE_STOP_FADE, TimelineKey};
use crate::timecode::{self, QuarterFrames, Rate, Timecode};
use crate::timeline::model;
use crate::web::WebServer;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// What a remote message asks for. Cues are named by their cue number.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Go,
    /// A button of a presenter-style remote (see `clicker`).
    Clicker(crate::clicker::Button),
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
    /// Get a cue ready ahead of time, and put the playhead on it.
    LoadCue(String),
    /// Hold a cue where it is, or let it go on.
    PauseCue(String),
    ResumeCue(String),
    /// Stop a cue at once and forget what was ready of it.
    ResetCue(String),
    /// Let a loop that is playing finish its pass and go on.
    Devamp(String),
    /// Stop everything and put the playhead back at the top of the list.
    Reset,
    /// GO on, or work, the cue list at this place in the tabs (from 1).
    GoList(u32),
    SelectList(u32),
    /// MIDI Time Code reached this position (every two frames while it runs).
    Timecode(Timecode, Rate),
    /// The timecode source located here (a full frame message).
    TimecodeLocate(Timecode, Rate),
    /// Something done to a timeline, named as a remote names it: a timeline
    /// list's name, or a timeline group's cue number (`ui::run_remote` finds it).
    Timeline {
        timeline: String,
        does: TimelineDo,
    },
}

/// A place on a timeline, as a remote gives it: seconds, or a marker's name.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Time(f64),
    Marker(String),
}

impl Place {
    /// A place written out: seconds (`12.5`), minutes and seconds (`1:02.5`,
    /// `1:00:00`), or else a marker's name. None for nothing at all.
    pub fn parse(text: &str) -> Option<Place> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        Some(match clock_seconds(t) {
            Some(secs) => Place::Time(secs),
            None => Place::Marker(t.to_string()),
        })
    }

    fn from_arg(arg: &OscValue) -> Option<Place> {
        match arg {
            OscValue::Number(n) if n.is_finite() => Some(Place::Time(*n)),
            OscValue::Number(_) => None,
            OscValue::Text(t) => Place::parse(t),
        }
    }
}

/// `12.5`, `1:02.5` or `1:00:00` as seconds; None if it is none of those.
fn clock_seconds(t: &str) -> Option<f64> {
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        // Only digits (and, in the seconds, one point): "1e3" or "inf" is a name.
        let ok = !p.is_empty()
            && p.chars().all(|c| c.is_ascii_digit() || (last && c == '.'))
            && p.matches('.').count() <= 1
            && *p != ".";
        if !ok {
            return None;
        }
        let v: f64 = p.parse().ok()?;
        total = total * 60.0 + v;
    }
    total.is_finite().then_some(total)
}

/// What a remote asks a timeline to do.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineDo {
    /// Run from its playhead (or go on from a pause), or from a place.
    Run(Option<Place>),
    /// What GO does to it: run, continue at a pause, leave a loop, or (while
    /// it runs, or a list stands at its end) nothing.
    Go,
    /// Stop its clock and hold what plays, as the transport's Pause does.
    Pause,
    /// Go on from a pause; nothing if it is not paused.
    Resume,
    /// Stop, fading out over this many seconds (None: as a timeline Stop cue
    /// does by default, `TIMELINE_STOP_FADE`; 0: a cut).
    Stop(Option<f64>),
    /// Move its playhead: running, it plays on from there; else it parks there.
    Jump(Place),
    /// Leave the loop it is playing, as GO would.
    LeaveLoop,
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
    if let Some((address, rest)) = osc_string(packet)
        && let Some(cmd) = osc_command(address, osc_first_arg(rest).as_ref())
    {
        out.push(cmd);
    }
}

/// An OSC message's first argument.
#[derive(Clone, Debug, PartialEq)]
pub enum OscValue {
    Number(f64),
    Text(String),
}

impl OscValue {
    /// As a number: a number, or text that is one.
    fn number(&self) -> Option<f64> {
        match self {
            OscValue::Number(n) => Some(*n),
            OscValue::Text(t) => t.trim().parse().ok(),
        }
    }
}

/// The first argument of an OSC message, if it has one: what follows the
/// address is a type tag string (`,if`) and then the arguments. Ints, floats,
/// doubles, 64-bit ints, true and false (as 1 and 0) and strings count.
fn osc_first_arg(rest: &[u8]) -> Option<OscValue> {
    let (tags, mut data) = osc_string(rest)?;
    let tags = tags.strip_prefix(',')?;
    let first = tags.chars().next()?;
    let take = |n: usize, data: &mut &[u8]| -> Option<Vec<u8>> {
        let bytes = data.get(..n)?.to_vec();
        *data = &data[n..];
        Some(bytes)
    };
    let n = match first {
        'T' => 1.0,
        'F' => 0.0,
        'i' => f64::from(i32::from_be_bytes(take(4, &mut data)?.try_into().ok()?)),
        'f' => f64::from(f32::from_be_bytes(take(4, &mut data)?.try_into().ok()?)),
        'd' => f64::from_be_bytes(take(8, &mut data)?.try_into().ok()?),
        'h' => i64::from_be_bytes(take(8, &mut data)?.try_into().ok()?) as f64,
        's' => return Some(OscValue::Text(osc_string(data)?.0.to_string())),
        _ => return None,
    };
    Some(OscValue::Number(n))
}

/// The command for an OSC message, in the shape of QLab's: an optional
/// `/workspace/<id>` in front (what Companion and Stream Deck modules send),
/// `/go`, `/stop`, `/pause`, `/resume`, `/panic`, `/reset`, `/playhead/<next|
/// previous|number>`, `/select/<next|previous|number>`, and `/cue/<number>/…`
/// (also `/cue/selected`, `/cue/playhead` and `/cue/active` for the cue
/// selected, at the playhead, or every one playing, and `/cue_id/<id>/…`) with
/// start, go, select, load, stop, hardStop, pause, resume, reset, devamp,
/// fade, panic, arm, disarm, and armed (with 1 or 0 as its argument).
/// Timelines: `/timeline/<list name|group's cue number>/…` (`timeline_command`).
pub fn osc_command(address: &str, arg: Option<&OscValue>) -> Option<Command> {
    let mut parts: Vec<&str> = address.split('/').skip(1).collect();
    // Whichever workspace it names is the one that is open.
    if parts.len() >= 3 && parts[0].eq_ignore_ascii_case("workspace") {
        parts.drain(..2);
    }
    let lower: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    let words: Vec<&str> = lower.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["go"] => Some(Command::Go),
        ["panic"] => Some(Command::Panic),
        ["stop"] => Some(Command::StopAll),
        ["pause"] => Some(Command::Pause),
        ["resume"] => Some(Command::Resume),
        ["reset"] => Some(Command::Reset),
        ["cuelist", n, "go"] => n.parse().ok().map(Command::GoList),
        ["cuelist", n, "select"] => n.parse().ok().map(Command::SelectList),
        // The name as written; a fourth part is the place (for senders that
        // can't send an argument): `/timeline/Act_1/jump/Interval`.
        ["timeline", _, verb] | ["timeline", _, verb, _] if !parts[1].is_empty() => {
            let place = parts.get(3).map(|p| OscValue::Text((*p).to_string()));
            let does = timeline_command(verb, place.as_ref().or(arg))?;
            Some(Command::Timeline {
                timeline: parts[1].to_string(),
                does,
            })
        }
        ["playhead" | "select", "next"] => Some(Command::PlayheadNext),
        ["playhead" | "select", "previous"] => Some(Command::PlayheadPrevious),
        // Cue numbers are matched as written, not lower-cased.
        ["playhead" | "select", _] if !parts[1].is_empty() => {
            Some(Command::Select(parts[1].to_string()))
        }
        ["cue" | "cue_id", _, action] if !parts[1].is_empty() => {
            // Which cue: a number as written, or one of the words for a
            // cue found by where it is, or (cue_id) the cue's own id.
            let n = if words[0] == "cue_id" {
                format!("#{}", parts[1])
            } else if matches!(words[1], "selected" | "playhead" | "active") {
                words[1].to_string()
            } else {
                parts[1].to_string()
            };
            match *action {
                "start" => Some(Command::Fire(n)),
                "go" => Some(Command::GoCue(n)),
                "select" => Some(Command::Select(n)),
                "load" => Some(Command::LoadCue(n)),
                "stop" | "hardstop" | "panic" => Some(Command::Stop(n)),
                "fade" => Some(Command::FadeOut(n)),
                "pause" => Some(Command::PauseCue(n)),
                "resume" => Some(Command::ResumeCue(n)),
                "reset" => Some(Command::ResetCue(n)),
                "devamp" => Some(Command::Devamp(n)),
                "arm" => Some(Command::Arm(n)),
                "disarm" => Some(Command::Disarm(n)),
                "armed" => arg.and_then(OscValue::number).map(|a| {
                    if a != 0.0 {
                        Command::Arm(n)
                    } else {
                        Command::Disarm(n)
                    }
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// What `/timeline/<name>/<verb>` asks for (`verb` lower-cased): `run` or
/// `start` (from the place given, if one is), `go`, `pause`, `resume` or
/// `continue`, `stop` (fading over the seconds given, else as the Stop cue
/// does), `hardStop` or `panic` (a cut), `jump`, `seek` or `marker` (to the
/// seconds or marker given), and `leaveLoop` or `devamp`. The web remote's
/// paths use the same words.
pub fn timeline_command(verb: &str, arg: Option<&OscValue>) -> Option<TimelineDo> {
    let place = || arg.and_then(Place::from_arg);
    Some(match verb {
        "run" | "start" => TimelineDo::Run(place()),
        "go" => TimelineDo::Go,
        "pause" => TimelineDo::Pause,
        "resume" | "continue" => TimelineDo::Resume,
        "stop" => TimelineDo::Stop(
            arg.and_then(OscValue::number)
                .filter(|f| f.is_finite())
                .map(|f| f.max(0.0)),
        ),
        "hardstop" | "panic" => TimelineDo::Stop(Some(0.0)),
        "jump" | "seek" | "marker" => TimelineDo::Jump(place()?),
        "leaveloop" | "leave_loop" | "devamp" => TimelineDo::LeaveLoop,
        _ => return None,
    })
}

// ----- Timelines ------------------------------------------------------------

/// A name as a remote may write it: case and the difference between a space
/// and an underscore don't count (an OSC address can't hold a space).
fn squash(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect::<String>()
        .to_lowercase()
}

/// A timeline named by its reference rather than its name: `list:<id>` (a
/// timeline list), `#<id>` or `cue_id:<id>` (a timeline group's own cue id).
/// These never clash, so they are what feedback falls back to.
fn by_reference(show: &Show, name: &str) -> Option<TimelineKey> {
    let s = squash(name);
    let key = if let Some(id) = s.strip_prefix('#').or_else(|| s.strip_prefix("cue_id:")) {
        TimelineKey::Group(CueId(id.parse().ok()?))
    } else {
        TimelineKey::List(s.strip_prefix("list:")?.parse().ok()?)
    };
    model::is_timeline(show, key).then_some(key)
}

/// Every timeline that answers to `name` (case and space/underscore aside):
/// the timeline lists of that name in the tabs' order, then the timeline
/// groups with that cue number in the cue list's order.
pub fn timelines_named(show: &Show, name: &str) -> Vec<TimelineKey> {
    let wanted = squash(name);
    if wanted.is_empty() {
        return Vec::new();
    }
    let lists = show
        .lists
        .iter()
        .filter(|l| squash(&l.name) == wanted)
        .map(|l| TimelineKey::List(l.id));
    let groups = show
        .cues
        .iter()
        .filter(|c| squash(&c.number) == wanted)
        .map(|c| TimelineKey::Group(c.id));
    lists
        .chain(groups)
        .filter(|k| model::is_timeline(show, *k))
        .collect()
}

/// The timeline a remote means by `name`. A reference (`list:<id>`, `#<id>`,
/// `cue_id:<id>`) first; else, of the timelines that answer to the name
/// (`timelines_named`), a list before a group (the first tab), and a group in
/// the list being worked before one in the main list before any other. So a
/// name two timelines share always means the same one while the show and the
/// worked list stay as they are; `run_timeline` says so in the log.
pub fn find_timeline(show: &Show, active_list: u32, name: &str) -> Option<TimelineKey> {
    if let Some(k) = by_reference(show, name) {
        return Some(k);
    }
    let named = timelines_named(show, name);
    let in_list = |list: u32| {
        named.iter().copied().find(|k| match k {
            TimelineKey::Group(id) => show.cue(*id).is_some_and(|c| c.list == list),
            TimelineKey::List(_) => false,
        })
    };
    named
        .iter()
        .copied()
        .find(|k| matches!(k, TimelineKey::List(_)))
        .or_else(|| in_list(active_list))
        .or_else(|| in_list(0))
        .or_else(|| named.first().copied())
}

/// How a remote names a timeline in what it is told (OSC feedback, the web
/// remote): a list by its name, a group by its cue number.
pub fn timeline_name(show: &Show, key: TimelineKey) -> String {
    match key {
        TimelineKey::List(n) => show.list_name(n),
        TimelineKey::Group(id) => show.cue(id).map_or_else(String::new, |c| c.number.clone()),
    }
}

/// A timeline's reference, which no name can clash with.
fn timeline_address_ref(key: TimelineKey) -> String {
    match key {
        TimelineKey::List(n) => format!("list:{n}"),
        TimelineKey::Group(id) => format!("cue_id:{}", id.0),
    }
}

/// The timeline's part of an OSC address (`/newq/timeline/<this>/…`): its name
/// (`timeline_name`, with what an address can't hold made `_`) when that names
/// it and nothing else, else its reference: `list:<id>` or `cue_id:<id>`. So
/// two timelines never share an address, and a command sent to the address
/// fed back reaches the one it came from.
pub fn timeline_address(show: &Show, key: TimelineKey) -> String {
    let name: String = timeline_name(show, key)
        .trim()
        .chars()
        .map(|c| {
            if c.is_whitespace() || "/#*,?[]{}".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clear = !name.is_empty()
        && by_reference(show, &name).is_none()
        && timelines_named(show, &name) == [key];
    if clear {
        name
    } else {
        timeline_address_ref(key)
    }
}

/// The time on the timeline a remote's place stands for: seconds as given, or
/// the marker of that name (case and spaces as in `find_timeline`).
pub fn place_time(show: &Show, key: TimelineKey, place: &Place) -> Option<f64> {
    match place {
        Place::Time(t) => t.is_finite().then_some(t.max(0.0)),
        Place::Marker(name) => {
            let wanted = squash(name);
            model::settings(show, key)?
                .markers
                .iter()
                .find(|m| squash(&m.name) == wanted)
                .map(|m| m.time)
        }
    }
}

/// Do what a remote asks of the timeline it names; the log says when there is
/// no such timeline or place, or nothing to do.
pub fn run_timeline(show: &Show, engine: &mut Engine, timeline: &str, does: &TimelineDo) {
    let Some(key) = find_timeline(show, engine.active_list, timeline) else {
        engine.log(format!("Remote: there is no timeline “{timeline}”"));
        return;
    };
    let named = timelines_named(show, timeline);
    if named.len() > 1 && by_reference(show, timeline).is_none() {
        engine.problem(format!(
            "Remote: {} timelines are called “{timeline}”; this went to {} ({}). \
             Rename one, or send {} to be sure",
            named.len(),
            model::name(show, key),
            timeline_address_ref(key),
            timeline_address_ref(key),
        ));
    }
    let place = |p: &Place, engine: &mut Engine| {
        let t = place_time(show, key, p);
        if t.is_none()
            && let Place::Marker(m) = p
        {
            engine.log(format!(
                "Remote: the timeline “{}” has no marker “{m}”",
                model::name(show, key)
            ));
        }
        t
    };
    // As GO does, running or going on lets go of the show's pause.
    let unpause = |engine: &mut Engine| {
        if engine.paused {
            engine.set_paused(false);
        }
    };
    match does {
        TimelineDo::Run(None) => {
            unpause(engine);
            engine.timeline_run(show, key);
        }
        TimelineDo::Run(Some(p)) => {
            if let Some(t) = place(p, engine) {
                engine.timeline_run_from(show, key, t);
            }
        }
        TimelineDo::Go => engine.timeline_go_one(show, key),
        // As the transport's Pause: everything holds, live inputs stay live.
        TimelineDo::Pause => {
            engine.timeline_pause(show, key, PauseFreeze::Everything, &[], false, false)
        }
        TimelineDo::Resume => engine.timeline_continue(show, key),
        TimelineDo::Stop(fade) => {
            engine.timeline_stop(show, key, fade.unwrap_or(TIMELINE_STOP_FADE))
        }
        TimelineDo::Jump(p) => {
            if let Some(t) = place(p, engine) {
                // As the editor's ruler: the very start shows its first frame.
                engine.timeline_seek(show, key, t.max(crate::engine::FIRST_FRAME), true);
            }
        }
        TimelineDo::LeaveLoop => engine.timeline_leave(show, key),
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
/// (GO, STOP, RESUME, LOAD, ALL_OFF and RESET) drives the transport.
#[cfg(test)]
pub fn parse_midi(msg: &[u8], channel: Option<u8>) -> Vec<Command> {
    parse_midi_for(msg, channel, None)
}

/// `parse_midi`, acting only on MIDI Show Control sent to `device` (or to
/// everyone, id 127) when one is given.
pub fn parse_midi_for(msg: &[u8], channel: Option<u8>, device: Option<u8>) -> Vec<Command> {
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
        // Another device's message is not for us.
        if device.is_some_and(|d| msg[2] != d && msg[2] != 0x7F) {
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
            (0x03, Some(n)) => vec![Command::ResumeCue(n)],
            (0x03, None) => vec![Command::Resume],
            (0x05, Some(n)) => vec![Command::LoadCue(n)],
            (0x08, _) => vec![Command::Panic],
            (0x0A, _) => vec![Command::Reset],
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
    device: Option<u8>,
    mtc: bool,
    quarter_frames: &mut QuarterFrames,
) -> Vec<Command> {
    if msg.first() == Some(&0xF1) {
        return match msg.get(1).and_then(|&b| quarter_frames.push(b)) {
            Some((tc, rate)) if mtc => vec![Command::Timecode(tc, rate)],
            _ => Vec::new(),
        };
    }
    parse_midi_for(msg, channel, device)
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
    /// The TCP port the web remote is serving on.
    pub web_port: Option<u16>,
    pub web_error: Option<String>,
}

/// Runs the listeners the show's settings ask for.
pub struct Remote {
    tx: Sender<Command>,
    rx: Receiver<Command>,
    wake: Arc<dyn Fn() + Send + Sync>,
    osc: Option<OscListener>,
    midi: Option<midir::MidiInputConnection<()>>,
    web: Option<WebServer>,
    /// The show's state as the web remote's page reads it.
    web_state: Arc<parking_lot::Mutex<String>>,
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
            web: None,
            web_state: Arc::new(parking_lot::Mutex::new("{}".into())),
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
        if first
            || old.web != wanted.web
            || old.web_port != wanted.web_port
            || old.web_pin != wanted.web_pin
        {
            self.web = None;
            self.status.web_port = None;
            self.status.web_error = None;
            if wanted.web {
                match WebServer::start(
                    wanted.web_port,
                    wanted.web_pin.clone(),
                    self.web_state.clone(),
                    self.tx.clone(),
                    self.wake.clone(),
                ) {
                    Ok(s) => {
                        self.status.web_port = Some(s.port);
                        self.web = Some(s);
                    }
                    Err(e) => self.status.web_error = Some(e),
                }
            }
        }
        // The channel filter applies in the callback, which reads it at connect
        // time, so a changed channel needs a reconnect too.
        if !first
            && (old.midi_channel != wanted.midi_channel || old.msc_device != wanted.msc_device)
            && wanted.midi
            && self.midi.is_some()
        {
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
        let device = wanted.msc_device;
        let mtc = wanted.mtc;
        let mut quarter_frames = QuarterFrames::default();
        input
            .connect(
                &port,
                "newq-in",
                move |_, msg, _| {
                    // The system's MIDI code calls this: a panic must not
                    // leave it (it would end the program on a Mac and on
                    // Windows), so a message that can't be read is dropped.
                    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        midi_commands(msg, channel, device, mtc, &mut quarter_frames)
                    }));
                    let Ok(commands) = read else { return };
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

    /// Whether the web remote is up, and so wants its state kept fresh.
    pub fn web_running(&self) -> bool {
        self.web.is_some()
    }

    /// Give the web remote's page the show's current state.
    pub fn set_web_state(&self, json: String) {
        *self.web_state.lock() = json;
    }

    /// A way for another thread to send commands, and to wake the loop for them.
    pub fn sender(&self) -> (Sender<Command>, Arc<dyn Fn() + Send + Sync>) {
        (self.tx.clone(), self.wake.clone())
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

/// A show with timelines for the remote's tests (here, in `feedback` and in
/// `web`): texts only, so no media is needed.
#[cfg(test)]
pub mod fixtures {
    use crate::engine::{Engine, RunState};
    use crate::show::*;
    use crate::timeline::Marker;
    use std::time::{Duration, Instant};

    pub fn text(show: &mut Show, end: f64) -> Cue {
        let mut c = show.make_cue(CueType::Text, None, show.cues.len());
        if let CueKind::Text(t) = &mut c.kind {
            t.text = "Hello".into();
            t.media.transition = Transition::Cut;
            t.media.fade_in = 0.0;
            t.media.fade_out = 0.0;
            t.media.end = Some(end);
        }
        c
    }

    pub fn control(show: &mut Show, c: ControlCue) -> Cue {
        let mut cue = show.make_cue(CueType::Control, None, show.cues.len());
        cue.kind = CueKind::Control(c);
        cue
    }

    /// A timeline list "Act 1" (a text from 0 to 6 s, a pause at 3 s that lets
    /// it run on, a text at 4 s; a marker "Middle" at 2 s) and, in the main
    /// list, a timeline group numbered "5" (a text from 0 to 4 s, a pause at 1 s
    /// that holds everything). Returns the show, the list and the group.
    pub fn show() -> (Show, u32, CueId) {
        let mut show = Show::default();
        let mut g = show.make_cue(CueType::Group, None, 0);
        g.number = "5".into();
        g.name = "Opening".into();
        if let CueKind::Group(gc) = &mut g.kind {
            gc.mode = GroupMode::Timeline;
        }
        let gid = g.id;
        show.cues.push(g);
        let mut c = text(&mut show, 4.0);
        c.parent = Some(gid);
        // On a layer above the list's: the two timelines play together on the
        // one display. On the same layer the group's text, once drawn (on a
        // thread, so whenever a busy machine gets to it), would replace the
        // list's and leave it nothing to fade at a Stop.
        if let CueKind::Text(t) = &mut c.kind {
            t.media.layer = 1;
        }
        show.cues.push(c);
        let mut p = control(
            &mut show,
            ControlCue {
                action: ControlAction::TimelinePause,
                freeze: PauseFreeze::Everything,
                ..Default::default()
            },
        );
        p.parent = Some(gid);
        p.pre_wait = 1.0;
        show.cues.push(p);
        let l = show.add_list("Act 1");
        show.lists.last_mut().unwrap().timeline = Some(TimelineSettings {
            markers: vec![Marker {
                id: 1,
                time: 2.0,
                name: "Middle".into(),
                color: None,
            }],
            ..TimelineSettings::default()
        });
        let mut bed = text(&mut show, 6.0);
        bed.list = l;
        show.cues.push(bed);
        let mut p = control(
            &mut show,
            ControlCue {
                action: ControlAction::TimelinePause,
                ..Default::default()
            },
        );
        p.list = l;
        p.pre_wait = 3.0;
        show.cues.push(p);
        let mut after = text(&mut show, 1.0);
        after.list = l;
        after.pre_wait = 4.0;
        show.cues.push(after);
        show.normalize_lists();
        (show, l, gid)
    }

    pub fn engine(show: &Show) -> Engine {
        let mut e = Engine::for_tests();
        e.output_sizes.insert(show.outputs[0].id, (64, 36));
        e
    }

    /// Tick (by `dt`, or by nothing while a text is still being drawn or the
    /// run cues) while `cond` holds; at most ten seconds.
    pub fn run_while(
        e: &mut Engine,
        show: &Show,
        k: TimelineKey,
        dt: f64,
        mut cond: impl FnMut(&mut Engine) -> bool,
    ) {
        let t0 = Instant::now();
        while cond(e) {
            assert!(t0.elapsed() < Duration::from_secs(10), "timed out");
            let busy = e.active.iter().any(|a| !a.started)
                || e.timeline(show, k).state == RunState::Cueing;
            if busy {
                std::thread::sleep(Duration::from_millis(1));
            }
            e.tick(show, if busy { 0.0 } else { dt });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::RunState;
    use crate::timeline::edit;

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
            ("/cue/3.5/load", Command::LoadCue("3.5".into())),
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

    /// An OSC message with one argument of the given type tag.
    fn osc_with(address: &str, tag: &str, arg: &[u8]) -> Vec<u8> {
        let mut v = address.as_bytes().to_vec();
        v.push(0);
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
        v.push(b',');
        v.extend_from_slice(tag.as_bytes());
        v.push(0);
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
        v.extend_from_slice(arg);
        v
    }

    #[test]
    fn osc_in_the_shape_of_qlabs_is_understood() {
        let cases = [
            // The workspace in front is whichever is open.
            ("/workspace/ABC-123/go", Command::Go),
            ("/workspace/ABC-123/cue/5/start", Command::Fire("5".into())),
            (
                "/workspace/x/cue/selected/stop",
                Command::Stop("selected".into()),
            ),
            ("/reset", Command::Reset),
            ("/select/next", Command::PlayheadNext),
            ("/select/previous", Command::PlayheadPrevious),
            ("/select/4", Command::Select("4".into())),
            ("/playhead/4.5", Command::Select("4.5".into())),
            ("/cue/playhead/start", Command::Fire("playhead".into())),
            ("/cue/active/pause", Command::PauseCue("active".into())),
            ("/cue/active/stop", Command::Stop("active".into())),
            ("/cue/3/hardStop", Command::Stop("3".into())),
            ("/cue/3/panic", Command::Stop("3".into())),
            ("/cue/3/pause", Command::PauseCue("3".into())),
            ("/cue/3/resume", Command::ResumeCue("3".into())),
            ("/cue/3/reset", Command::ResetCue("3".into())),
            ("/cue/3/devamp", Command::Devamp("3".into())),
            ("/cuelist/2/go", Command::GoList(2)),
            ("/workspace/X/cuelist/3/select", Command::SelectList(3)),
            ("/cue/3/load", Command::LoadCue("3".into())),
            ("/cue_id/12345/start", Command::Fire("#12345".into())),
        ];
        for (addr, want) in cases {
            assert_eq!(parse_osc(&osc(addr)), vec![want], "{addr}");
        }
        // `armed` takes its answer from the argument, whatever type it is.
        let one = 1i32.to_be_bytes();
        let zero = 0.0f32.to_be_bytes();
        assert_eq!(
            parse_osc(&osc_with("/cue/2/armed", "i", &one)),
            vec![Command::Arm("2".into())]
        );
        assert_eq!(
            parse_osc(&osc_with("/cue/2/armed", "f", &zero)),
            vec![Command::Disarm("2".into())]
        );
        assert_eq!(
            parse_osc(&osc_with("/cue/2/armed", "T", &[])),
            vec![Command::Arm("2".into())]
        );
        assert_eq!(
            parse_osc(&osc_with("/cue/2/armed", "F", &[])),
            vec![Command::Disarm("2".into())]
        );
        assert_eq!(
            parse_osc(&osc_with("/cue/2/armed", "s", b"1\0\0\0")),
            vec![Command::Arm("2".into())]
        );
        // Without an answer it isn't a message about arming.
        assert!(parse_osc(&osc("/cue/2/armed")).is_empty());
        assert!(parse_osc(&osc_with("/cue/2/armed", "i", &[0, 0])).is_empty());
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
            vec![Command::LoadCue("9".into())]
        );
        assert_eq!(
            parse_midi(&msc(0x03, b"6"), None),
            vec![Command::ResumeCue("6".into())]
        );
        assert_eq!(parse_midi(&msc(0x0A, b""), None), vec![Command::Reset]);
        assert_eq!(parse_midi(&msc(0x08, b""), None), vec![Command::Panic]);
        // Commands NewQ doesn't act on, and other sysex.
        assert!(parse_midi(&msc(0x0B, b""), None).is_empty());
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
                got.extend(midi_commands(&[0xF1, b], None, None, mtc, &mut q));
            }
            got.extend(midi_commands(&full, None, None, mtc, &mut q));
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
            midi_commands(&[0x90, 60, 100], None, None, false, &mut q),
            vec![Command::Note(60)]
        );
        assert!(parse_midi(&[0xF1, 0x00], None).is_empty());
    }

    #[test]
    fn show_control_for_another_device_is_left_alone() {
        // Sent to device 5 (the id is the third byte).
        let mut to_five = msc(0x01, b"2");
        to_five[2] = 5;
        let mut to_all = msc(0x01, b"2");
        to_all[2] = 0x7F;
        let go = vec![Command::GoCue("2".into())];
        assert_eq!(parse_midi_for(&to_five, None, None), go);
        assert_eq!(parse_midi_for(&to_five, None, Some(5)), go);
        assert!(parse_midi_for(&to_five, None, Some(6)).is_empty());
        // Everyone is everyone.
        assert_eq!(parse_midi_for(&to_all, None, Some(6)), go);
        // Notes and the like don't have device ids.
        assert_eq!(
            parse_midi_for(&[0x90, 60, 100], None, Some(6)),
            vec![Command::Note(60)]
        );
    }

    #[test]
    fn the_listeners_apply_the_show_control_device() {
        let mut q = QuarterFrames::default();
        let mut to_five = msc(0x01, b"2");
        to_five[2] = 5;
        assert_eq!(
            midi_commands(&to_five, None, Some(5), false, &mut q),
            vec![Command::GoCue("2".into())]
        );
        assert!(midi_commands(&to_five, None, Some(9), false, &mut q).is_empty());
    }

    fn tl(name: &str, does: TimelineDo) -> Vec<Command> {
        vec![Command::Timeline {
            timeline: name.into(),
            does,
        }]
    }

    #[test]
    fn timeline_addresses_map_to_commands() {
        use TimelineDo::*;
        let marker = |m: &str| Place::Marker(m.into());
        let cases = [
            ("/timeline/Act_1/run", tl("Act_1", Run(None))),
            ("/timeline/5/start", tl("5", Run(None))),
            ("/timeline/5/go", tl("5", Go)),
            ("/timeline/5/pause", tl("5", Pause)),
            ("/timeline/5/resume", tl("5", Resume)),
            ("/timeline/5/continue", tl("5", Resume)),
            ("/timeline/5/stop", tl("5", Stop(None))),
            ("/timeline/5/hardStop", tl("5", Stop(Some(0.0)))),
            ("/timeline/5/panic", tl("5", Stop(Some(0.0)))),
            ("/timeline/5/leaveLoop", tl("5", LeaveLoop)),
            ("/timeline/5/devamp", tl("5", LeaveLoop)),
            // The place as one more part of the address.
            (
                "/timeline/5/jump/Interval",
                tl("5", Jump(marker("Interval"))),
            ),
            (
                "/timeline/5/marker/Interval",
                tl("5", Jump(marker("Interval"))),
            ),
            ("/timeline/5/seek/1:02.5", tl("5", Jump(Place::Time(62.5)))),
            ("/timeline/5/run/12", tl("5", Run(Some(Place::Time(12.0))))),
            // The workspace in front, the name as written, the verb in any case.
            ("/workspace/W/timeline/Act 1/PAUSE", tl("Act 1", Pause)),
        ];
        for (addr, want) in cases {
            assert_eq!(parse_osc(&osc(addr)), want, "{addr}");
        }
        // The place or the fade as the argument.
        let f = |v: f32| v.to_be_bytes();
        assert_eq!(
            parse_osc(&osc_with("/timeline/5/jump", "f", &f(12.5))),
            tl("5", Jump(Place::Time(12.5)))
        );
        assert_eq!(
            parse_osc(&osc_with("/timeline/5/jump", "i", &7i32.to_be_bytes())),
            tl("5", Jump(Place::Time(7.0)))
        );
        assert_eq!(
            parse_osc(&osc_with("/timeline/5/jump", "s", b"Interval\0\0\0\0")),
            tl("5", Jump(marker("Interval")))
        );
        assert_eq!(
            parse_osc(&osc_with("/timeline/5/start", "s", b"2:00\0\0\0\0")),
            tl("5", Run(Some(Place::Time(120.0))))
        );
        assert_eq!(
            parse_osc(&osc_with("/timeline/5/stop", "f", &f(1.5))),
            tl("5", Stop(Some(1.5)))
        );
        assert_eq!(
            parse_osc(&osc_with("/timeline/5/stop", "f", &f(-2.0))),
            tl("5", Stop(Some(0.0))),
            "no fade less than none"
        );
        // A jump needs a place; nonsense is ignored.
        for addr in [
            "/timeline/5/jump",
            "/timeline/5/dance",
            "/timeline//run",
            "/timeline/5",
            "/timeline/5/jump/a/b",
        ] {
            assert!(parse_osc(&osc(addr)).is_empty(), "{addr}");
        }
        assert!(parse_osc(&osc_with("/timeline/5/jump", "f", &f(f32::NAN))).is_empty());
    }

    #[test]
    fn places_are_seconds_clock_times_or_marker_names() {
        assert_eq!(Place::parse("12.5"), Some(Place::Time(12.5)));
        assert_eq!(Place::parse(" 1:02.5 "), Some(Place::Time(62.5)));
        assert_eq!(Place::parse("1:00:00"), Some(Place::Time(3600.0)));
        assert_eq!(Place::parse("0"), Some(Place::Time(0.0)));
        for name in [
            "Interval", "inf", "1e3", "1:2:3:4", ":5", "1.2.3", "-3", ".",
        ] {
            assert_eq!(
                Place::parse(name),
                Some(Place::Marker(name.into())),
                "{name}"
            );
        }
        assert_eq!(Place::parse("  "), None);
    }

    #[test]
    fn names_two_timelines_share_resolve_one_way_and_feedback_addresses_stay_apart() {
        let (mut show, l, g) = fixtures::show();
        let list = TimelineKey::List(l);
        let group = TimelineKey::Group(g);
        // Alone, each keeps its name.
        assert_eq!(timeline_address(&show, list), "Act_1");
        assert_eq!(timeline_address(&show, group), "5");
        // A second timeline list "act 1", and a timeline list named "5" like
        // the group's number.
        let l2 = show.add_list("act 1");
        edit::set_list_timeline(&mut show, l2, true).unwrap();
        let l5 = show.add_list("5");
        edit::set_list_timeline(&mut show, l5, true).unwrap();
        let (list2, list5) = (TimelineKey::List(l2), TimelineKey::List(l5));
        assert_eq!(timelines_named(&show, "Act_1"), vec![list, list2]);
        assert_eq!(timelines_named(&show, "5"), vec![list5, group]);
        // The rule: a list before a group, the first tab first, whichever list
        // is worked.
        for worked in [0, l, l2, l5] {
            assert_eq!(find_timeline(&show, worked, "ACT_1"), Some(list));
            assert_eq!(find_timeline(&show, worked, "5"), Some(list5));
        }
        // Each one's address is its reference now, and finds it.
        let all = [list, list2, list5, group];
        let addresses: Vec<String> = all.iter().map(|k| timeline_address(&show, *k)).collect();
        assert_eq!(
            addresses,
            vec![
                format!("list:{l}"),
                format!("list:{l2}"),
                format!("list:{l5}"),
                format!("cue_id:{}", g.0)
            ]
        );
        for (k, a) in all.iter().zip(&addresses) {
            assert_eq!(find_timeline(&show, 0, a), Some(*k), "{a}");
            assert!(!a.contains(['#', ' ', '/']), "{a} in an OSC address");
        }
        assert_eq!(find_timeline(&show, 0, &format!("#{}", g.0)), Some(group));
        // A list named like a reference doesn't take the reference over.
        let tricky = show.add_list(&format!("list:{l}"));
        edit::set_list_timeline(&mut show, tricky, true).unwrap();
        assert_eq!(find_timeline(&show, 0, &format!("list:{l}")), Some(list));
        assert_eq!(
            timeline_address(&show, TimelineKey::List(tricky)),
            format!("list:{tricky}")
        );
        // A command by a shared name says which it went to.
        let mut e = fixtures::engine(&show);
        run_timeline(&show, &mut e, "5", &TimelineDo::Stop(Some(0.0)));
        let said: Vec<String> = e.messages.iter().map(|m| m.1.clone()).collect();
        let said = said.join("\n");
        assert!(said.contains("2 timelines are called “5”"), "{said}");
        assert!(said.contains(&format!("list:{l5}")), "{said}");
        let n = e.messages.len();
        run_timeline(
            &show,
            &mut e,
            &format!("cue_id:{}", g.0),
            &TimelineDo::Stop(Some(0.0)),
        );
        assert!(
            !e.messages
                .iter()
                .skip(n)
                .any(|m| m.1.contains("are called")),
            "a reference is never ambiguous"
        );
        // Renamed apart, the names come back.
        let i = show.lists.iter().position(|x| x.id == l2).unwrap();
        show.lists[i].name = "Act 2".into();
        assert_eq!(timeline_address(&show, list), "Act_1");
        assert_eq!(timeline_address(&show, list2), "Act_2");
    }

    #[test]
    fn a_remote_finds_a_timeline_by_list_name_or_group_number() {
        let (mut show, l, g) = fixtures::show();
        let list = TimelineKey::List(l);
        let group = TimelineKey::Group(g);
        for name in ["Act 1", "act_1", "ACT 1", " Act_1 ", &format!("list:{l}")] {
            assert_eq!(find_timeline(&show, 0, name), Some(list), "{name}");
        }
        assert_eq!(find_timeline(&show, 0, "5"), Some(group));
        assert_eq!(find_timeline(&show, l, "5"), Some(group), "from any list");
        assert_eq!(find_timeline(&show, 0, &format!("#{}", g.0)), Some(group));
        for none in ["", "Act 2", "list:0", "list:99", "#1", "99", "#x"] {
            assert_eq!(find_timeline(&show, 0, none), None, "{none:?}");
        }
        // A list that is not a timeline is none, and the number of a cue that
        // is not a timeline group neither.
        let plain = show.add_list("Notes");
        assert_eq!(find_timeline(&show, 0, "Notes"), None);
        assert_eq!(find_timeline(&show, 0, &format!("list:{plain}")), None);
        let first_text = show.children(g)[0].number.clone();
        assert_eq!(find_timeline(&show, 0, &first_text), None);
        // How it is named back: a list by its name, a group by its number.
        assert_eq!(timeline_name(&show, list), "Act 1");
        assert_eq!(timeline_name(&show, group), "5");
        // Places: seconds as given (never before 0), markers by name.
        assert_eq!(place_time(&show, list, &Place::Time(-3.0)), Some(0.0));
        assert_eq!(
            place_time(&show, list, &Place::Marker("middle".into())),
            Some(2.0)
        );
        assert_eq!(place_time(&show, list, &Place::Marker("End".into())), None);
        assert_eq!(
            place_time(&show, group, &Place::Marker("Middle".into())),
            None
        );
    }

    fn said(e: &Engine, what: &str) -> bool {
        e.messages.iter().any(|(_, m)| m.contains(what))
    }

    #[test]
    fn a_remote_runs_pauses_continues_jumps_and_stops_a_timeline_list() {
        let (show, l, _) = fixtures::show();
        let k = TimelineKey::List(l);
        let mut e = fixtures::engine(&show);
        let state = |e: &mut Engine| e.timeline(&show, k).state;
        // Run, while the main list is the one worked.
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Run(None));
        assert_eq!(state(&mut e), RunState::Running);
        fixtures::run_while(&mut e, &show, k, 0.05, |e| e.timeline(&show, k).pos < 0.5);
        // GO aimed at it while it runs does nothing, and says so.
        let before = e.timeline(&show, k).pos;
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Go);
        assert_eq!(state(&mut e), RunState::Running);
        assert_eq!(e.timeline(&show, k).pos, before);
        assert!(said(&e, "GO does nothing until it pauses"));
        // Pause holds it; resume (or GO) goes on.
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Pause);
        assert!(matches!(state(&mut e), RunState::Paused { .. }));
        e.tick(&show, 0.5);
        assert_eq!(e.timeline(&show, k).pos, before, "the clock stands");
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Resume);
        assert_eq!(state(&mut e), RunState::Running);
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Resume);
        assert!(said(&e, "not paused"));
        // A jump to a marker, by name, plays on from there.
        run_timeline(
            &show,
            &mut e,
            "act 1",
            &TimelineDo::Jump(Place::Marker("MIDDLE".into())),
        );
        fixtures::run_while(&mut e, &show, k, 0.0, |e| {
            e.timeline(&show, k).state == RunState::Cueing
        });
        let pos = e.timeline(&show, k).pos;
        assert!((pos - 2.0).abs() < 0.01, "{pos}");
        assert_eq!(state(&mut e), RunState::Running);
        run_timeline(
            &show,
            &mut e,
            "Act_1",
            &TimelineDo::Jump(Place::Marker("Nowhere".into())),
        );
        assert!(said(&e, "has no marker “Nowhere”"));
        // To its pause at 3 s, where GO aimed at it continues it.
        fixtures::run_while(&mut e, &show, k, 0.05, |e| {
            !matches!(e.timeline(&show, k).state, RunState::Paused { .. })
        });
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Go);
        assert_eq!(state(&mut e), RunState::Running);
        // Stop fades (3 s, as the Stop cue), a second Stop cuts.
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Stop(None));
        let v = e.timeline(&show, k);
        assert!(v.state == RunState::Stopped && v.fading, "{v:?}");
        e.tick(&show, crate::show::TIMELINE_STOP_FADE - 0.5);
        assert!(e.timeline(&show, k).fading, "still fading");
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Stop(None));
        assert!(!e.is_active(), "cut");
        // Run from a place; a hard stop cuts at once.
        run_timeline(
            &show,
            &mut e,
            "Act_1",
            &TimelineDo::Run(Some(Place::Time(1.0))),
        );
        fixtures::run_while(&mut e, &show, k, 0.0, |e| {
            e.timeline(&show, k).state == RunState::Cueing
        });
        assert_eq!(state(&mut e), RunState::Running);
        assert!((e.timeline(&show, k).pos - 1.0).abs() < 0.01);
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Stop(Some(0.0)));
        assert!(!e.is_active());
        // No loop to leave; no such timeline.
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::LeaveLoop);
        assert!(said(&e, "not playing a loop"));
        run_timeline(&show, &mut e, "Act 9", &TimelineDo::Run(None));
        assert!(said(&e, "there is no timeline “Act 9”"));
        // A jump of a stopped timeline parks it there.
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Jump(Place::Time(0.0)));
        e.tick(&show, 0.0);
        let v = e.timeline(&show, k);
        assert!(v.parked && v.pos > 0.0 && v.pos < 0.01, "{v:?}");
    }

    #[test]
    fn a_remote_goes_on_with_a_timeline_group_and_leaves_a_loop() {
        use crate::show::{ControlAction, ControlCue, TimelinePoint};
        let (mut show, l, g) = fixtures::show();
        let gk = TimelineKey::Group(g);
        let mut e = fixtures::engine(&show);
        // GO aimed at the group fires it; it stops at its pause.
        run_timeline(&show, &mut e, "5", &TimelineDo::Go);
        fixtures::run_while(&mut e, &show, gk, 0.05, |e| {
            !matches!(e.timeline(&show, gk).state, RunState::Paused { .. })
        });
        run_timeline(&show, &mut e, "5", &TimelineDo::Go);
        assert_eq!(e.timeline(&show, gk).state, RunState::Running);
        run_timeline(&show, &mut e, "5", &TimelineDo::Stop(Some(0.0)));
        // A loop on the list: leaving it lets this pass be the last.
        let lp = fixtures::control(
            &mut show,
            ControlCue {
                action: ControlAction::TimelinePause,
                loop_from: Some(TimelinePoint::Time(0.5)),
                ..Default::default()
            },
        );
        let lp_id = lp.id;
        show.cues.push(lp);
        let c = show.cue_mut(lp_id).unwrap();
        c.list = l;
        c.pre_wait = 1.5;
        show.normalize_lists();
        let k = TimelineKey::List(l);
        run_timeline(&show, &mut e, "Act 1", &TimelineDo::Run(None));
        fixtures::run_while(&mut e, &show, k, 0.05, |e| {
            e.timeline(&show, k).looping.is_none()
        });
        run_timeline(&show, &mut e, "Act 1", &TimelineDo::LeaveLoop);
        assert!(e.timeline(&show, k).looping.is_some_and(|l| l.last_pass));
    }
}
