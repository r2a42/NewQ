//! Line-up across the show network (mapping plan 4.4): the primary, where the
//! operator sits, holds the points and solves; the computer that drives the
//! projector only draws crosses where it is told, and stops by itself.
//!
//! - `LineUpMsg` is what goes over a session as `Msg::LineUp`. `Show` is the
//!   whole state every time (latest wins: no deltas, so a lost or late
//!   message costs nothing and a reconnect is "send it again"); `End` is the
//!   end of a line-up; `Seen` comes back: the crosses are up, on a monitor
//!   this big. An older NewQ reads `Msg::LineUp` as `Msg::Unknown` and a
//!   later kind inside it as `LineUpMsg::Unknown`; neither closes a session.
//! - `Sender` (the primary) decides when to send: at once on a change (sent
//!   urgent), once a second with nothing changing (`REPEAT`), the outline at
//!   most five times a second (`OUTLINE_EVERY`). It hears `Seen` back and
//!   says how the link stands (`Link`, `link_words`).
//! - `Shown` (the projector's computer) holds the last `Overlay` and when it
//!   came; it is dropped `LEASE` (5 s) after the last `Show`, measured as
//!   time since it came, never by taking time from an `Instant`.
//! - `route` is the trust rule, pure: `Show` and `End` only from the primary
//!   this computer follows (the hub already hands on only heeded sessions);
//!   `Seen` only on a primary, and only for the session and the computer the
//!   `Sender` is for.
//!
//! One path alone and networked: with the projector's display on the
//! primary itself, the `Sender` is for `Target::Here` and its messages go
//! straight into this computer's own `Shown` (`Source::Here`), whose `Seen`
//! goes back into the `Sender`.
//!
//! Everything takes `now` as an argument: tests run on a clock they own.

use crate::net::node::NodeId;
use crate::show::OutputId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A model's outline: pieces of lines, in pixels.
pub type Outline = Vec<Vec<[f32; 2]>>;

/// A held overlay is dropped this long after the last `Show` for it.
pub const LEASE: Duration = Duration::from_secs(5);
/// With nothing changing, the primary sends the whole state again this often.
pub const REPEAT: Duration = Duration::from_secs(1);
/// The model's outline (a whole-screen picture to draw again) at most this
/// often.
pub const OUTLINE_EVERY: Duration = Duration::from_millis(200);
/// No `Seen` this long after the first `Show` to a connected computer: it
/// runs a NewQ that doesn't know line-up.
pub const SEEN_WITHIN: Duration = Duration::from_secs(3);
/// At most this many marks are drawn.
pub const MAX_MARKS: usize = 99;
/// At most this many outline pieces, and points in all of them.
pub const MAX_OUTLINE_PIECES: usize = 256;
pub const MAX_OUTLINE_POINTS: usize = 16_384;
/// Pictures larger than this either way aren't any monitor's.
pub const MAX_PIXELS: u32 = 1 << 15;

/// A line-up message (`Msg::LineUp`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum LineUpMsg {
    /// Primary to the display's computer: show exactly this.
    Show(Box<Overlay>),
    /// Primary to that computer: this line-up is over.
    End {
        #[serde(default)]
        session: u64,
    },
    /// Back: this is shown, on a monitor of this size.
    Seen {
        #[serde(default)]
        session: u64,
        #[serde(default)]
        seq: u64,
        #[serde(default)]
        pixels: (u32, u32),
    },
    /// A kind a later NewQ sends: passed by.
    #[serde(other)]
    Unknown,
}

/// What a projector shows while it is lined up: crosses in raw monitor
/// pixels over black (or its picture dimmed), and the model's outline as the
/// present solve throws it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    /// A random number per line-up, so a stale message is passed by.
    #[serde(default)]
    pub session: u64,
    /// Counts up within a session; one older than the last is dropped.
    #[serde(default)]
    pub seq: u64,
    #[serde(default = "no_display")]
    pub display: OutputId,
    /// The picture's size the marks are in (pixels).
    #[serde(default)]
    pub pixels: (u32, u32),
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
    /// The mark the operator is moving (an index into `marks`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<u8>,
    /// The model's edges as the present solve throws them, in pieces, in the
    /// same pixels. None: keep the last one of this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Vec<Vec<[f32; 2]>>>,
    #[serde(default)]
    pub under: Under,
}

fn no_display() -> OutputId {
    OutputId(0)
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

/// One cross: the point's number as the operator sees it, where it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// The point's number (index + 1, switched-off points counted).
    #[serde(default)]
    pub n: u16,
    /// Pixels from the picture's top-left (x right, y down).
    #[serde(default)]
    pub at: [f32; 2],
    /// Switched off: drawn dim.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
}

/// What is under the crosses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Under {
    #[default]
    Black,
    /// The display's usual picture, dimmed.
    Dimmed,
    /// A later NewQ's: drawn as black.
    #[serde(other)]
    Unknown,
}

impl Overlay {
    /// The same overlay with numbers that can't be drawn left out: marks
    /// that aren't numbers, more marks or outline than are drawn, an active
    /// mark that isn't there. None for a picture with no size (or larger than
    /// any monitor): nothing can be placed in it.
    pub fn sane(mut self) -> Option<Overlay> {
        let (w, h) = self.pixels;
        if w == 0 || h == 0 || w > MAX_PIXELS || h > MAX_PIXELS {
            return None;
        }
        let finite = |p: &[f32; 2]| p.iter().all(|v| v.is_finite() && v.abs() < 1e6);
        self.marks.retain(|m| finite(&m.at));
        self.marks.truncate(MAX_MARKS);
        if self.active.is_some_and(|a| a as usize >= self.marks.len()) {
            self.active = None;
        }
        if let Some(o) = &mut self.outline {
            for piece in o.iter_mut() {
                piece.retain(finite);
            }
            o.retain(|p| p.len() >= 2);
            o.truncate(MAX_OUTLINE_PIECES);
            let mut left = MAX_OUTLINE_POINTS;
            for piece in o.iter_mut() {
                piece.truncate(left);
                left -= piece.len();
            }
            o.retain(|p| p.len() >= 2);
        }
        Some(self)
    }

    /// Its contents without the session, the count or the outline: what a
    /// change is judged by.
    fn body(&self) -> (OutputId, (u32, u32), &[Mark], Option<u8>, Under) {
        (
            self.display,
            self.pixels,
            &self.marks,
            self.active,
            self.under,
        )
    }
}

/// Where a held overlay came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// This computer's own line-up (alone, or the primary's own display).
    Here,
    /// The primary this computer follows.
    Primary(NodeId),
}

/// Where a line-up's crosses are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// On this computer.
    Here,
    /// On that computer of the network.
    Computer(NodeId),
}

/// What `play_message` does with a line-up message (`route`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// To this computer's `Shown`.
    Shown,
    /// To this primary's `Sender` (which checks the session and computer).
    Sender,
    /// Passed by.
    Pass,
}

/// The trust rule (mapping plan 4.4), on top of the hub's (only a heeded
/// session is handed on): `Show` and `End` only from the primary this
/// computer follows (`from_my_primary`, as `play_message`'s `primary`);
/// `Seen` only on a primary (`leading`); anything else passed by. A primary
/// never takes crosses from a follower.
pub fn route(msg: &LineUpMsg, from_my_primary: bool, leading: bool) -> Route {
    match msg {
        LineUpMsg::Show(_) | LineUpMsg::End { .. } if from_my_primary => Route::Shown,
        LineUpMsg::Seen { .. } if leading => Route::Sender,
        _ => Route::Pass,
    }
}

/// What the projector's computer holds.
struct Held {
    source: Source,
    overlay: Overlay,
    /// When the last `Show` for it came.
    at: Instant,
}

/// The projector's computer: the overlay it shows, and its lease.
#[derive(Default)]
pub struct Shown {
    held: Option<Held>,
    /// The last outline (out of `held`, so a `Show` without one keeps it).
    outline: Option<Arc<Outline>>,
    /// Counts each outline taken: the picture is drawn again when it moves.
    outline_rev: u64,
}

impl Shown {
    /// A line-up message, already routed here (`route`). `monitor` gives the
    /// size of the monitor display `id` is shown on here (None: not a
    /// display of this computer's, and the message is passed by). Returns
    /// the `Seen` to send back for a `Show` taken.
    pub fn take(
        &mut self,
        source: Source,
        msg: LineUpMsg,
        monitor: impl Fn(OutputId) -> Option<(u32, u32)>,
        now: Instant,
    ) -> Option<LineUpMsg> {
        match msg {
            LineUpMsg::Show(o) => {
                let size = monitor(o.display)?;
                let mut o = o.sane()?;
                if let Some(h) = &self.held
                    && h.source == source
                    && h.overlay.session == o.session
                    && o.seq <= h.overlay.seq
                {
                    // Late: a newer one is shown already.
                    return None;
                }
                let same = self
                    .held
                    .as_ref()
                    .is_some_and(|h| h.source == source && h.overlay.session == o.session);
                match o.outline.take() {
                    Some(outline) => {
                        self.outline = Some(Arc::new(outline));
                        self.outline_rev += 1;
                    }
                    None if same => {}
                    None => {
                        if self.outline.take().is_some() {
                            self.outline_rev += 1;
                        }
                    }
                }
                let (session, seq) = (o.session, o.seq);
                self.held = Some(Held {
                    source,
                    overlay: o,
                    at: now,
                });
                Some(LineUpMsg::Seen {
                    session,
                    seq,
                    pixels: size,
                })
            }
            LineUpMsg::End { session } => {
                if self
                    .held
                    .as_ref()
                    .is_some_and(|h| h.source == source && h.overlay.session == session)
                {
                    self.clear();
                }
                None
            }
            LineUpMsg::Seen { .. } | LineUpMsg::Unknown => None,
        }
    }

    /// The lease: the overlay is dropped once `LEASE` has passed since its
    /// last `Show`. Whether it was dropped now.
    pub fn tick(&mut self, now: Instant) -> bool {
        let out = self
            .held
            .as_ref()
            .is_some_and(|h| now.saturating_duration_since(h.at) > LEASE);
        if out {
            self.clear();
        }
        out
    }

    /// Dropped whatever it is (this computer left the network, changed role
    /// or primary). Whether there was one.
    pub fn clear(&mut self) -> bool {
        if self.outline.take().is_some() {
            self.outline_rev += 1;
        }
        self.held.take().is_some()
    }

    /// Dropped when it came from a primary this computer doesn't follow now
    /// (`primary`: the one it follows, None for none). This computer's own
    /// is kept. Whether it was dropped.
    pub fn keep_only(&mut self, primary: Option<NodeId>) -> bool {
        let stale = self
            .held
            .as_ref()
            .is_some_and(|h| matches!(h.source, Source::Primary(p) if Some(p) != primary));
        stale && self.clear()
    }

    /// The overlay to draw, if any (its `outline` is always None: see
    /// `outline`).
    pub fn overlay(&self) -> Option<&Overlay> {
        self.held.as_ref().map(|h| &h.overlay)
    }

    /// Where it came from.
    #[cfg(test)]
    pub fn source(&self) -> Option<Source> {
        self.held.as_ref().map(|h| h.source)
    }

    /// The outline to draw, and its count (a new count: draw it again).
    pub fn outline(&self) -> Option<(&Arc<Outline>, u64)> {
        self.held.as_ref()?;
        self.outline.as_ref().map(|o| (o, self.outline_rev))
    }

    /// What a window compares to know it must draw again: the session, the
    /// count and the outline's count.
    pub fn stamp(&self) -> Option<(u64, u64, u64)> {
        self.held
            .as_ref()
            .map(|h| (h.overlay.session, h.overlay.seq, self.outline_rev))
    }
}

/// How the link to the crosses stands, on the primary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Sent, nothing back yet.
    Waiting,
    /// Shown there, on a monitor this big.
    Seen { pixels: (u32, u32) },
    /// The computer isn't connected.
    Lost,
    /// Connected, but nothing comes back: it runs a NewQ without line-up.
    Older,
}

/// The primary's side of one line-up: what to send, and when.
pub struct Sender {
    pub session: u64,
    pub display: OutputId,
    pub target: Target,
    seq: u64,
    /// The last `Show` sent (its outline left out), and when.
    last: Option<Overlay>,
    sent_at: Option<Instant>,
    first_sent: Option<Instant>,
    /// The outline's count last sent, and when.
    outline_sent: Option<u64>,
    outline_at: Option<Instant>,
    /// The last `Seen`: for which count, the monitor's size, when.
    seen: Option<(u64, (u32, u32), Instant)>,
}

impl Sender {
    pub fn new(session: u64, display: OutputId, target: Target) -> Sender {
        Sender {
            session,
            display,
            target,
            seq: 0,
            last: None,
            sent_at: None,
            first_sent: None,
            outline_sent: None,
            outline_at: None,
            seen: None,
        }
    }

    /// The `Show` to send now for `want` (its session, count and display are
    /// this sender's; its outline is `outline`, counted by `outline_rev`), or
    /// None when nothing is due: sent at once when it changed, the whole of
    /// it again `REPEAT` after the last, the outline at most every
    /// `OUTLINE_EVERY` (a change held back is sent when its time comes).
    pub fn due(
        &mut self,
        want: &Overlay,
        outline: Option<&Vec<Vec<[f32; 2]>>>,
        outline_rev: u64,
        now: Instant,
    ) -> Option<LineUpMsg> {
        let mut body = Overlay {
            session: self.session,
            seq: 0,
            display: self.display,
            outline: None,
            ..want.clone()
        };
        let changed = self.last.as_ref().is_none_or(|l| l.body() != body.body());
        let repeat = self
            .sent_at
            .is_none_or(|t| now.saturating_duration_since(t) >= REPEAT);
        let outline_new = outline.is_some() && self.outline_sent != Some(outline_rev);
        let outline_may = self
            .outline_at
            .is_none_or(|t| now.saturating_duration_since(t) >= OUTLINE_EVERY);
        let send_outline = outline.is_some() && (repeat || (outline_new && outline_may));
        if !(changed || repeat || send_outline) {
            return None;
        }
        self.seq += 1;
        body.seq = self.seq;
        self.last = Some(body.clone());
        self.sent_at = Some(now);
        self.first_sent.get_or_insert(now);
        if send_outline {
            body.outline = outline.cloned();
            self.outline_sent = Some(outline_rev);
            self.outline_at = Some(now);
        }
        Some(LineUpMsg::Show(Box::new(body)))
    }

    /// The message that ends it.
    pub fn end(&self) -> LineUpMsg {
        LineUpMsg::End {
            session: self.session,
        }
    }

    /// A message routed to the sender (`route`), from `from`: a `Seen` for
    /// this line-up from the computer it is for is taken; anything else is
    /// passed by. Whether it was taken.
    pub fn heard(&mut self, from: Target, msg: &LineUpMsg, now: Instant) -> bool {
        match *msg {
            LineUpMsg::Seen {
                session,
                seq,
                pixels,
            } if from == self.target && session == self.session && seq <= self.seq => {
                if self.seen.is_none_or(|(s, ..)| seq >= s) {
                    self.seen = Some((seq, pixels, now));
                }
                true
            }
            _ => false,
        }
    }

    /// How the link stands: `connected`, whether the computer has a session
    /// now (true for `Target::Here`).
    pub fn link(&self, connected: bool, now: Instant) -> Link {
        if self.target != Target::Here && !connected {
            return Link::Lost;
        }
        match (self.seen, self.first_sent) {
            (Some((_, pixels, _)), _) => Link::Seen { pixels },
            (None, Some(t)) if now.saturating_duration_since(t) > SEEN_WITHIN => Link::Older,
            _ => Link::Waiting,
        }
    }
}

/// The link in words for the line-up panes, if there is something to say:
/// `computer` the computer's name ("this computer" for one's own),
/// `version` the NewQ it runs, `points_in` the picture size the points are
/// placed in.
pub fn link_words(
    link: Link,
    computer: &str,
    version: &str,
    points_in: (u32, u32),
) -> Option<String> {
    match link {
        Link::Waiting => None,
        Link::Lost => Some(format!(
            "Lost the link to {computer}. The crosses there aren't following. Your points are \
             kept."
        )),
        Link::Older => Some(format!(
            "{computer} runs NewQ {version}, which can't show line-up crosses. Update it, or \
             line this projector up on that computer."
        )),
        Link::Seen { pixels } if pixels != points_in => Some(format!(
            "{computer} shows this projector's display at {} × {}, but the points are placed in \
             a picture {} × {}: set the projector's pixels to its monitor's first.",
            pixels.0, pixels.1, points_in.0, points_in.1
        )),
        Link::Seen { .. } => None,
    }
}

#[cfg(test)]
#[path = "lineup_tests.rs"]
mod tests;
