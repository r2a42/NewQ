//! Where the 3D stage's screens are (mapping plan 4.2): run-time state, as a
//! playing cue's opacity is, never written into `Show::stage3d` (that would
//! make the scene and every lens map again, and resend the show on a show
//! network). A screen cue moves a screen; where it is outlives the cue.
//!
//! - Each of a screen's six numbers (x, y, z, turn, tilt, roll) moves on its
//!   own, by the engine's `Ramp` (`Param`), from where it was when its cue
//!   began. A cue that gives only some numbers leaves the others alone, even
//!   mid-move by another cue.
//! - A move keeps to its cue's time, not to a sum of ticks: while the cue
//!   plays, its time is the cue's own (`ActiveCue::elapsed`), which a shared
//!   clock anchors, a late command's backdating moves and a timeline's clock
//!   carries. So every computer of a show network has the screen at the same
//!   place at the same shared time.
//! - Once the cue is gone (it ended, or *Stop all*, *Panic* or a stop let it
//!   go) the move goes on to where it was told by the same clock (its last
//!   anchor): NewQ doesn't drive the scenery, and stopping the sound doesn't
//!   stop a winch. Nothing moves a screen back but another cue or *Put at*.
//! - On a show network the primary's heartbeat carries every moved screen
//!   (`PlayState::screens`): where it is and each number on its way. A
//!   follower that has never compared takes them at once; afterwards it acts
//!   only on a difference seen at `STEADY_BEATS` heartbeats in a row, and then
//!   eases to the primary's place over `EASE` (R21: the picture never jumps
//!   at the primary).

use super::{Engine, Param};
use crate::net::play::{AxisMove, Command, PlayState, ScreenNow};
use crate::show::screen::{AXES, axes_of, placement_of};
use crate::show::{Cue, Curve, ScreenCue, Show};
use crate::stage3d::{Placement, SurfaceId};
use std::collections::{BTreeMap, HashMap};

/// How long a screen cue takes (a time that isn't one, or less than 0, is a
/// jump).
pub fn length(s: &ScreenCue) -> f64 {
    if s.duration.is_finite() {
        s.duration.max(0.0)
    } else {
        0.0
    }
}

/// A follower put right about a screen eases to the primary's place over
/// this long (seconds), never jumping.
pub const EASE: f64 = 0.5;
/// Two places closer than this (metres, and degrees) are the same place.
pub const SAME_METRES: f32 = 0.002;
pub const SAME_DEGREES: f32 = 0.05;
/// A screen is put right only once it has differed at this many heartbeats
/// in a row (about a second, as cues are: `engine/net.rs`).
const STEADY_BEATS: u32 = 4;

/// The time a move (or an ease) keeps: its cue's while that plays, then its
/// own by the same clock.
#[derive(Clone, Copy, Debug)]
struct Clock {
    /// The cue's instance whose time it is; 0 (no instance has it) for one
    /// taken from a heartbeat.
    instance: u64,
    elapsed: f64,
    /// The shared time its time was 0, while ticking by a shared clock.
    anchor: Option<f64>,
}

impl Clock {
    fn free(elapsed: f64, anchor: Option<f64>) -> Clock {
        Clock {
            instance: 0,
            elapsed,
            anchor,
        }
    }
}

/// One of a screen's numbers.
#[derive(Clone, Debug)]
struct Axis {
    param: Param,
    /// While it moves.
    clock: Option<Clock>,
}

impl Axis {
    fn at(v: f32) -> Axis {
        Axis {
            param: Param::new(v),
            clock: None,
        }
    }

    /// From where it is to `to` over `secs` along `curve`, `clock`'s time in.
    fn go(&mut self, from: f32, to: f32, secs: f64, curve: Curve, clock: Clock) {
        self.param = Param::new(from);
        self.param.ramp_to(to, secs, curve);
        self.param.ramp_at(clock.elapsed);
        self.clock = self.param.ramping().then_some(clock);
    }
}

/// A follower easing to the primary's place: the numbers it eased from, each
/// of them till its new move takes it over.
#[derive(Clone, Debug)]
struct Ease {
    from: [Option<f32>; AXES],
    clock: Clock,
}

#[derive(Clone, Debug)]
struct Screen {
    axes: [Axis; AXES],
    ease: Option<Ease>,
}

impl Screen {
    fn at(p: &Placement) -> Screen {
        Screen {
            axes: axes_of(p).map(Axis::at),
            ease: None,
        }
    }

    /// Where it is, without the ease.
    fn true_place(&self) -> [f32; AXES] {
        std::array::from_fn(|i| self.axes[i].param.value)
    }

    /// Where it is, the ease included.
    fn shown(&self) -> [f32; AXES] {
        let mut v = self.true_place();
        if let Some(e) = &self.ease {
            let k = Curve::SCurve.apply(e.clock.elapsed / EASE) as f32;
            for (i, from) in e.from.iter().enumerate() {
                if let Some(f) = from {
                    v[i] = f + (v[i] - f) * k;
                }
            }
        }
        v
    }

    fn moving(&self) -> bool {
        self.ease.is_some() || self.axes.iter().any(|a| a.clock.is_some())
    }
}

/// Every moved screen. A screen not here is at home (its placement in the
/// show).
#[derive(Default)]
pub struct Screens {
    at: BTreeMap<SurfaceId, Screen>,
}

/// What a follower has seen of the primary's screens (in `NetCorrection`).
#[derive(Default, Debug)]
pub struct ScreenDoubts {
    /// It has compared a heartbeat's screens once: from then on, a
    /// difference is eased away, not taken at once.
    heard: bool,
    /// The heartbeats in a row each screen has differed at.
    steady: HashMap<SurfaceId, u32>,
}

impl ScreenDoubts {
    pub fn is_empty(&self) -> bool {
        self.steady.is_empty()
    }
}

fn same(a: &[f32; AXES], b: &[f32; AXES]) -> bool {
    (0..AXES).all(|i| {
        let tol = if i < 3 { SAME_METRES } else { SAME_DEGREES };
        // (A number that isn't one is never the same: it is put right.)
        (a[i] - b[i]).abs() <= tol
    })
}

/// Where a heartbeat's screen is at shared time `now`.
fn theirs_at(s: &ScreenNow, now: f64) -> [f32; AXES] {
    let mut v = axes_of(&s.at);
    for m in &s.moving {
        let i = usize::from(m.axis);
        if i >= AXES || !m.from.is_finite() || !m.to.is_finite() {
            continue;
        }
        let k = if m.duration.is_finite() && m.duration > 0.0 {
            m.curve.apply((now - m.anchor) / m.duration) as f32
        } else {
            1.0
        };
        v[i] = m.from + (m.to - m.from) * k;
    }
    v
}

impl Engine {
    /// Where screen `id` is now, if a cue (or *Put at*) moved it; None: at
    /// home, its placement in the show.
    pub fn screen_at(&self, id: SurfaceId) -> Option<Placement> {
        self.screens.at.get(&id).map(|s| placement_of(s.shown()))
    }

    /// Every moved screen and where it is now, by id: what the outputs, the
    /// 3D views and the light draw them at (`StageLive::send`).
    pub fn moved_screens(&self) -> Vec<(SurfaceId, Placement)> {
        self.screens
            .at
            .iter()
            .map(|(k, v)| (*k, placement_of(v.shown())))
            .collect()
    }

    /// Whether a screen is on its way somewhere (the main loop keeps drawing).
    pub fn screens_moving(&self) -> bool {
        self.screens.at.values().any(Screen::moving)
    }

    /// *Put at*: screen `screen` is at `at` at once (a jump, no ramp). An
    /// input from outside the engine: on a show network's primary it is a
    /// command, done at the same shared time everywhere.
    pub fn put_screen(&mut self, show: &Show, screen: SurfaceId, at: Placement) {
        if self.gated(|_| Command::PutScreen { screen, at }) {
            return;
        }
        if show.stage3d.surface(screen).is_none() || !axes_of(&at).iter().all(|v| v.is_finite()) {
            return;
        }
        self.screens.at.insert(screen, Screen::at(&at));
    }

    /// Screen cue `cue` (`s`, playing as `instance`, `from` seconds in) has
    /// started: each number it gives moves from where the screen is now.
    pub(super) fn screen_cue(
        &mut self,
        show: &Show,
        cue: &Cue,
        s: &ScreenCue,
        instance: u64,
        from: f64,
    ) {
        let Some(id) = s.screen else {
            self.log(format!("Cue {}: no screen chosen to move", cue.number));
            return;
        };
        let Some(home) = show.stage3d.surface(id).map(|x| x.placement) else {
            self.log(format!(
                "Cue {}: its screen isn't on the 3D stage",
                cue.number
            ));
            return;
        };
        let screen = self
            .screens
            .at
            .entry(id)
            .or_insert_with(|| Screen::at(&home));
        let now = screen.shown();
        let secs = length(s);
        let clock = Clock {
            instance,
            elapsed: from.max(0.0),
            anchor: None,
        };
        for (i, to) in s.to.axes().into_iter().enumerate() {
            let Some(to) = to.filter(|v| v.is_finite()) else {
                continue;
            };
            screen.axes[i].go(now[i], to, secs, s.curve, clock);
            // This number is the new move's: the ease lets go of it.
            if let Some(e) = &mut screen.ease {
                e.from[i] = None;
            }
        }
    }

    /// Each tick, after the cues' times: every move to its cue's time (or
    /// its own once the cue is gone). A screen the show no longer has, or that
    /// no cue moves any more, is at home (a lens map has it there).
    pub(super) fn screens_tick(&mut self, show: &Show, dt: f64) {
        if self.screens.at.is_empty() {
            return;
        }
        let movable = show.movable_screens();
        self.screens
            .at
            .retain(|id, _| movable.contains(id) && show.stage3d.surface(*id).is_some());
        let shared = self.sync.map(|s| s.now);
        let paused = self.paused;
        let cues: HashMap<u64, (f64, Option<f64>)> = self
            .active
            .iter()
            .map(|a| (a.instance, (a.elapsed, a.anchor)))
            .collect();
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let step = |c: &mut Clock| {
            if let Some((elapsed, anchor)) = cues.get(&c.instance).filter(|_| c.instance != 0) {
                c.elapsed = *elapsed;
                c.anchor = *anchor;
                return;
            }
            // Its cue is gone: on by the clock it kept to.
            c.instance = 0;
            match (shared, &mut c.anchor) {
                (Some(_), Some(a)) if paused => *a += dt,
                (Some(now), Some(a)) => c.elapsed = (now - *a).max(c.elapsed),
                _ if !paused => c.elapsed += dt,
                _ => {}
            }
        };
        for s in self.screens.at.values_mut() {
            for a in &mut s.axes {
                if let Some(c) = &mut a.clock {
                    step(c);
                    a.param.ramp_at(c.elapsed);
                    if !a.param.ramping() {
                        a.clock = None;
                    }
                }
            }
            if let Some(e) = &mut s.ease {
                step(&mut e.clock);
                if e.clock.elapsed >= EASE || e.from.iter().all(Option::is_none) {
                    s.ease = None;
                }
            }
        }
    }

    /// The moved screens for the heartbeat, at shared time `now`.
    pub(super) fn screens_now(&self, now: f64) -> Vec<ScreenNow> {
        self.screens
            .at
            .iter()
            .map(|(id, s)| ScreenNow {
                screen: *id,
                at: placement_of(s.true_place()),
                moving: s
                    .axes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, a)| {
                        let c = a.clock?;
                        let (from, to, duration, curve) = a.param.ramp_parts()?;
                        Some(AxisMove {
                            axis: i as u8,
                            from,
                            to,
                            anchor: c.anchor.unwrap_or(now - c.elapsed),
                            duration,
                            curve,
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    /// Compare the screens with the primary's heartbeat `s` at shared time
    /// `now`, and put right what has differed long enough (see the module's
    /// words). What was put right, in words.
    pub(super) fn screens_correct(
        &mut self,
        show: &Show,
        s: &PlayState,
        now: f64,
        doubts: &mut ScreenDoubts,
    ) -> Vec<String> {
        let theirs: BTreeMap<SurfaceId, &ScreenNow> =
            s.screens.iter().map(|x| (x.screen, x)).collect();
        let mut ids: Vec<SurfaceId> = theirs.keys().copied().collect();
        ids.extend(self.screens.at.keys().copied());
        ids.sort_unstable();
        ids.dedup();
        let first = !doubts.heard;
        doubts.heard = true;
        let mut done = Vec::new();
        let mut steady = HashMap::new();
        for id in ids {
            let Some(home) = show.stage3d.surface(id).map(|x| x.placement) else {
                continue;
            };
            let want = theirs
                .get(&id)
                .map_or_else(|| axes_of(&home), |t| theirs_at(t, now));
            let ours = self
                .screens
                .at
                .get(&id)
                .map_or_else(|| axes_of(&home), Screen::shown);
            if same(&want, &ours) {
                continue;
            }
            let beats = doubts.steady.get(&id).copied().unwrap_or(0) + 1;
            if !first && beats < STEADY_BEATS {
                steady.insert(id, beats);
                continue;
            }
            // Theirs, exactly: each number where it is and on its way.
            let mut fresh = Screen::at(&theirs.get(&id).map_or(home, |t| t.at));
            for m in theirs.get(&id).map_or(&[][..], |t| &t.moving[..]) {
                let i = usize::from(m.axis);
                if i < AXES && m.from.is_finite() && m.to.is_finite() && m.anchor.is_finite() {
                    let elapsed = (now - m.anchor).max(0.0);
                    fresh.axes[i].go(
                        m.from,
                        m.to,
                        m.duration,
                        m.curve,
                        Clock::free(elapsed, Some(m.anchor)),
                    );
                }
            }
            // Just joined: nothing was right before, so it is taken at once.
            // Else eased to, from where the picture has it.
            if !first {
                fresh.ease = Some(Ease {
                    from: ours.map(Some),
                    clock: Clock::free(0.0, Some(now)),
                });
            }
            self.screens.at.insert(id, fresh);
            let name = show
                .stage3d
                .surface(id)
                .map_or_else(|| "a screen".to_string(), |x| x.name.clone());
            done.push(if first {
                format!("{name} put where the primary has it")
            } else {
                format!("{name} eased to where the primary has it")
            });
        }
        doubts.steady = steady;
        done
    }

    /// The shared clock was put back or on by `by` (`net_shift_shared`).
    pub(super) fn screens_shift(&mut self, by: f64) {
        for s in self.screens.at.values_mut() {
            let clocks = s
                .axes
                .iter_mut()
                .filter_map(|a| a.clock.as_mut())
                .chain(s.ease.as_mut().map(|e| &mut e.clock));
            for c in clocks {
                if let Some(a) = &mut c.anchor {
                    *a += by;
                }
            }
        }
    }

    /// Back to ticking by `dt` alone (`net_leave_shared`).
    pub(super) fn screens_unanchor(&mut self) {
        for s in self.screens.at.values_mut() {
            for a in &mut s.axes {
                if let Some(c) = &mut a.clock {
                    c.anchor = None;
                }
            }
            if let Some(e) = &mut s.ease {
                e.clock.anchor = None;
            }
        }
    }

    /// A show was opened: every screen is at home.
    pub(super) fn screens_home(&mut self) {
        self.screens = Screens::default();
    }
}

#[cfg(test)]
#[path = "screens_tests.rs"]
mod tests;
