//! Line-up (projection plan 5.3, step P3): from points on the model and
//! where each is seen in a projector's picture, where the projector really
//! is, which way it points and, given enough points, its lens. Pure f64
//! sums, hand-written (no linear algebra crate): no GPU, UI or files.
//!
//! Conventions (the same as `stage3d::projector`'s, tested against
//! `Projector::in_picture` and `ray`):
//! - **The room**: metres, right-handed, Y up. The solver's point on the
//!   model is `RoomPoint::world`.
//! - **The picture**: a point's place in the projector's picture is in
//!   **pixels** (`RoomPoint::pixel`), `(0, 0)` the picture's top-left corner
//!   and `(W, H)` its bottom-right, x right and y down; pixel `(i, j)`'s
//!   middle is `(i + 0.5, j + 0.5)`. `W` and `H` are `Problem::pixels` (the
//!   picture's size the points were placed at). That is `in_picture`'s
//!   `(u, v)` times `(W, H)`. Errors are in these pixels.
//! - **The lens** (`LensFit`) is `Lens`'s: throw = distance / width, shift a
//!   share of the picture (x right, y up), aspect the picture's width per
//!   height (`Projector::aspect`: the lens's own, else `W / H`). Bounds are
//!   `MIN_THROW..MAX_THROW`, `±MAX_SHIFT` and 0.2 to 10, as there.
//! - **The pose** (`Pose`): the lens's position and `Placement`'s yaw, pitch
//!   and roll (degrees, `Ry * Rx * Rz`), the lens looking down its own −Z.
//!   Kept in f64; `Pose::placement` rounds to the show's f32. Looking
//!   straight up or down, roll comes out 0 and the turn is all yaw.
//! - **Point numbers**: an index in data is into `Problem::points` (0 for
//!   the first, switched-off points counted); words say index + 1, the
//!   number the operator sees.
//!
//! What is solved (`Rung`), as 5.3's table: under four points, nothing
//! (refused). Four or more: the pose, with the lens as entered. Six or more
//! not all on one flat surface: the throw too. Eight or more, not flat: the
//! shift too. The picture's shape (aspect, an anamorphic lens) only when
//! the caller frees it (`Freedom::aspect`) and the shift was found. A rung
//! is used only if the points pin its numbers down (each to about 2% for a
//! pixel's error per point) and they come out inside a lens's bounds;
//! `Solution::notes` says why one wasn't.
//!
//! Method: starting guesses (the caller's `Problem::start`, a flat set's
//! homography, the 3x4 DLT of six or more points that aren't flat; a fixed
//! ring of places round the points facing them only when those leave the
//! points over a pixel out and there is no start or no linear guess), each
//! given `model::START_ROUNDS` of Levenberg-Marquardt on the error in pixels
//! (pose only, lens as entered), the best taken on to at most
//! `model::MAX_ROUNDS` (`Solution::capped` says if it got there still
//! moving). Then the rungs from the top down: the highest the points and
//! `Freedom` allow, tried from that pose and from the DLT's lens, judged
//! after `START_ROUNDS`, a rung lower each time one isn't pinned down, the
//! one kept refined to the end. Then each point's error, and with five or
//! more points, one a pixel or more out, a leave-one-out of the three
//! furthest off: the point whose removal leaves the others agreeing best,
//! named when it stands out. A fit over 5 px that would be good with the
//! picture flipped left to right (a mirror, a projector's rear setting) is
//! refused (not on a flat set, where a flipped picture fits from behind).
//! Everything is bounded; no clock.
//!
//! The types that are kept are the show's (`stage3d::projector::{LineUp,
//! LinePoint, Solved, Before, Rung}`): a point is kept in its screen's own
//! frame (`LinePoint::at` with `surface`), or the room's for a venue corner.
//! This module works in room metres only. `to_room` is the one way from a
//! kept point to the room (and `to_screen` the way back, for a point placed
//! on a screen); `Problem::for_line_up` uses it for every point and carries
//! the line-up's `projector::points_key` (the one fingerprint, worked out
//! from the kept numbers) through to `Solution::points_key`, so
//! `LineUp::is_current` agrees with what was solved.
//!
//! Use: build a `Problem` (`Problem::for_line_up` from a projector, its
//! line-up and the stage; or `Problem::new` from a projector's lens, the
//! picture's size in pixels and points already in room metres), call
//! `solve`, and show `Solution::words` or `Refusal::words`. Apply with
//! `Solution::placement`, `Solution::lens_onto` and keep `Solution::solved`.
//! `project` is the forward sum on the show's own types, the very one the
//! solver fits (for the overlay's outline and each point's error).
//! `fit_rigid` is the rigid fit (turn and move between two sets of points)
//! for lining up a screen or a tracker later. Everything is serde, so a
//! `Solution` can be carried as it is.
//!
//! Determinism: no clock, no threads, no hashing by address and loops in a
//! fixed order, so the same `Problem` gives the same `Solution` on the same
//! computer, bit for bit. Only the primary solves, so nothing is promised
//! between systems (another's `sin` or `atan2` may differ in the last bit).

#[cfg(test)]
mod tests;

mod linalg;
mod model;
mod start;

use crate::render3d::camera::MAX_SHIFT;
pub(crate) use crate::stage3d::projector::Rung;
use crate::stage3d::projector::{self, Lens, LineUp, MAX_THROW, MIN_THROW, Projector, Solved};
use crate::stage3d::{Placement, Stage3d, SurfaceId};
use linalg::{M3, V3};
use model::{Pair, State};
use serde::{Deserialize, Serialize};

/// The narrowest and widest picture shape a solve may give a lens (width /
/// height; `Lens::aspect`'s bounds).
pub(crate) const MIN_ASPECT: f64 = 0.2;
pub(crate) const MAX_ASPECT: f64 = 10.0;
/// The shapes drawn at all (`projector::sane_aspect`'s bounds): an entered
/// aspect (the pixels') is kept within these, as NewQ draws it.
const MIN_DRAWN_ASPECT: f64 = 0.1;
const MAX_DRAWN_ASPECT: f64 = 20.0;

/// The fewest points anything is solved from, and for each lens number.
pub(crate) const POSE_POINTS: usize = 4;
pub(crate) const THROW_POINTS: usize = 6;
pub(crate) const SHIFT_POINTS: usize = 8;

/// A set is flat when its thinnest axis is under this share of its widest
/// (as standard deviations): one screen, or one wall.
const FLAT: f64 = 0.05;
/// A set is in a line when its second axis is under this share of its widest.
const IN_A_LINE: f64 = 0.01;
/// How many of the points furthest off a leave-one-out tries.
const LEAVE_OUT_TRIES: usize = 3;
/// Two points closer than this (metres) are the same spot.
const SAME_SPOT: f64 = 1e-3;
/// Two points closer than this in the picture (pixels) are at the same place.
const SAME_PIXEL: f64 = 0.5;
/// How well a lens number must be pinned down to be used: its standard
/// deviation, a pixel's error per point, as a share (throw, aspect) or in
/// pictures (shift).
const SURE: f64 = 0.02;
/// A point stands out when, solved without it, it is at least this many
/// pixels off and this many times the others' error.
const STANDS_OUT_PX: f64 = 3.0;
const STANDS_OUT_TIMES: f64 = 5.0;
/// Fits better than this are "within half a pixel".
const HALF: f64 = 0.5;
/// Over this (RMS) a flipped picture is looked for: a mirrored rig fits
/// tens of pixels out, a wrong lens shift a few.
const MIRROR_PX: f64 = 5.0;
/// Over this the shift is worth asking about when it wasn't solved.
const POOR_PX: f64 = 2.0;

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

/// One pair as the solver takes it: a spot in the room and where it is in
/// the picture. Made from a kept `projector::LinePoint` by `to_room` (never
/// kept itself: the show keeps `projector::LinePoint`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoomPoint {
    /// Metres, in the room.
    #[serde(default)]
    pub world: [f64; 3],
    /// Pixels in the projector's picture (the module's words).
    #[serde(default)]
    pub pixel: [f64; 2],
    /// Switched off: kept, numbered, not used.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
}

impl RoomPoint {
    pub fn new(world: [f64; 3], pixel: [f64; 2]) -> RoomPoint {
        RoomPoint {
            world,
            pixel,
            on: true,
        }
    }
}

/// A kept point's place in the room (metres): `at` through its screen's
/// placement (`Placement::matrix`, as the scene places the screen) when it
/// is on one, else `at` itself. The one way from what the show keeps to
/// what the solver takes. None when its screen is gone.
pub(crate) fn to_room(stage: &Stage3d, p: &projector::LinePoint) -> Option<[f64; 3]> {
    let at = p.at.map(|v| v as f64);
    let Some(id) = p.surface else {
        return Some(at);
    };
    let pose = Pose::of(&stage.surface(id)?.placement);
    Some(linalg::add(linalg::mul_v(&pose.turn(), at), pose.position))
}

/// The way back: room position `room` in screen `surface`'s own frame (None
/// when there is no such screen), or as it is for no screen. What a point
/// placed on a screen is kept as.
pub(crate) fn to_screen(
    stage: &Stage3d,
    surface: Option<SurfaceId>,
    room: [f64; 3],
) -> Option<[f32; 3]> {
    let Some(id) = surface else {
        return Some(room.map(|v| v as f32));
    };
    let pose = Pose::of(&stage.surface(id)?.placement);
    let r = pose.turn();
    let d = linalg::sub(room, pose.position);
    // A turn's inverse is its transpose.
    let back = [0, 1, 2].map(|k| r[0][k] * d[0] + r[1][k] * d[1] + r[2][k] * d[2]);
    Some(back.map(|v| v as f32))
}

/// Every point of `line_up` in the room, in its order (switched-off points
/// too). Refused when a point that is on is on a screen that is gone (one
/// switched off stays numbered, at the room's origin).
pub(crate) fn room_points(stage: &Stage3d, line_up: &LineUp) -> Result<Vec<RoomPoint>, Refusal> {
    line_up
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let world = match to_room(stage, p) {
                Some(w) => w,
                None if !p.on => [0.0; 3],
                None => return Err(Refusal::NoScreen { point: i }),
            };
            Ok(RoomPoint {
                world,
                pixel: p.pixel.map(|v| v as f64),
                on: p.on,
            })
        })
        .collect()
}

/// A lens, as the solver holds it (`Lens`'s numbers in f64).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct LensFit {
    #[serde(default = "default_throw")]
    pub throw: f64,
    #[serde(default)]
    pub shift: [f64; 2],
    #[serde(default = "default_aspect")]
    pub aspect: f64,
}

fn default_throw() -> f64 {
    crate::stage3d::projector::DEFAULT_THROW as f64
}

fn default_aspect() -> f64 {
    16.0 / 9.0
}

impl Default for LensFit {
    fn default() -> Self {
        LensFit {
            throw: default_throw(),
            shift: [0.0; 2],
            aspect: default_aspect(),
        }
    }
}

/// `v` within `lo..=hi` (constant bounds), `or` when it isn't a number.
fn within(v: f64, lo: f64, hi: f64, or: f64) -> f64 {
    if v.is_finite() { v.clamp(lo, hi) } else { or }
}

impl LensFit {
    /// A projector's lens for a picture of `pixels`, as NewQ draws it.
    pub fn of(p: &Projector, pixels: (u32, u32)) -> LensFit {
        LensFit {
            throw: p.lens.throw() as f64,
            shift: p.lens.shift().map(|v| v as f64),
            aspect: crate::stage3d::projector::sane_aspect(p.aspect(pixels)) as f64,
        }
        .sane()
    }

    /// Kept within a lens's bounds.
    pub fn sane(self) -> LensFit {
        let s = MAX_SHIFT as f64;
        LensFit {
            throw: within(
                self.throw,
                MIN_THROW as f64,
                MAX_THROW as f64,
                default_throw(),
            ),
            shift: self.shift.map(|v| within(v, -s, s, 0.0)),
            // As `sane_aspect`: outside, the 16:9 NewQ draws instead.
            aspect: if (MIN_DRAWN_ASPECT..=MAX_DRAWN_ASPECT).contains(&self.aspect) {
                self.aspect
            } else {
                default_aspect()
            },
        }
    }
}

/// Where a projector's lens is and which way it is turned, in f64.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pose {
    /// Metres.
    #[serde(default)]
    pub position: [f64; 3],
    /// Degrees, as `Placement`'s.
    #[serde(default)]
    pub yaw: f64,
    #[serde(default)]
    pub pitch: f64,
    #[serde(default)]
    pub roll: f64,
}

impl Pose {
    pub fn of(p: &Placement) -> Pose {
        let f = |v: f32| if v.is_finite() { v as f64 } else { 0.0 };
        Pose {
            position: p.position.map(f),
            yaw: f(p.yaw),
            pitch: f(p.pitch),
            roll: f(p.roll),
        }
    }

    /// Rounded to the show's numbers.
    pub fn placement(&self) -> Placement {
        Placement {
            position: self.position.map(|v| v as f32),
            yaw: self.yaw as f32,
            pitch: self.pitch as f32,
            roll: self.roll as f32,
        }
    }

    /// The lens's frame to the room (row-major), as `Placement::matrix`.
    fn turn(&self) -> M3 {
        linalg::from_euler(self.yaw, self.pitch, self.roll)
    }

    fn from_turn(r: &M3, position: V3) -> Pose {
        let (yaw, pitch, roll) = linalg::to_euler(r);
        Pose {
            position,
            yaw,
            pitch,
            roll,
        }
    }

    fn sane(&self) -> bool {
        self.position
            .iter()
            .chain([&self.yaw, &self.pitch, &self.roll])
            .all(|v| v.is_finite())
    }
}

/// Which lens numbers the solver may work out (where the points allow).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Freedom {
    #[serde(default = "yes")]
    pub throw: bool,
    #[serde(default = "yes")]
    pub shift: bool,
    /// An anamorphic lens: off unless asked for.
    #[serde(default)]
    pub aspect: bool,
}

impl Default for Freedom {
    fn default() -> Self {
        Freedom {
            throw: true,
            shift: true,
            aspect: false,
        }
    }
}

/// Everything a solve needs, and nothing else: the same `Problem` gives the
/// same `Solution`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Problem {
    #[serde(default)]
    pub points: Vec<RoomPoint>,
    /// The picture's width and height in pixels.
    #[serde(default)]
    pub pixels: (u32, u32),
    /// The lens as entered: used as it is, or where the solve starts.
    #[serde(default)]
    pub lens: LensFit,
    /// Where the projector is thought to be (its placement now), if known.
    #[serde(default)]
    pub start: Option<Pose>,
    #[serde(default)]
    pub free: Freedom,
    /// The kept points' fingerprint (`projector::points_key`), handed on to
    /// `Solution::points_key`. Set by `for_line_up`; 0 for points made here.
    #[serde(default)]
    pub key: u64,
}

impl Problem {
    /// The problem for `projector` with its picture `pixels` big, starting
    /// from where it stands now, from points already in room metres.
    pub fn new(projector: &Projector, pixels: (u32, u32), points: Vec<RoomPoint>) -> Problem {
        Problem {
            points,
            pixels,
            lens: LensFit::of(projector, pixels),
            start: Some(Pose::of(&projector.placement)),
            free: Freedom::default(),
            key: 0,
        }
    }

    /// The problem for `projector`'s line-up as the show keeps it: each
    /// point turned into the room by `to_room`, the line-up's picture size,
    /// and its `points_key`.
    pub fn for_line_up(
        projector: &Projector,
        line_up: &LineUp,
        stage: &Stage3d,
    ) -> Result<Problem, Refusal> {
        let points = room_points(stage, line_up)?;
        Ok(Problem {
            key: line_up.points_key(),
            ..Problem::new(projector, line_up.pixels, points)
        })
    }
}

/// A lens number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LensPart {
    Throw,
    Shift,
    Aspect,
    #[serde(other)]
    Unknown,
}

impl LensPart {
    fn words(self) -> &'static str {
        match self {
            LensPart::Throw => "throw",
            LensPart::Shift => "lens shift",
            LensPart::Aspect => "picture's shape",
            LensPart::Unknown => "lens",
        }
    }
}

/// Why a lens number wasn't worked out, or something else to know.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "note")]
pub(crate) enum Note {
    /// The points are all on one flat surface.
    Flat,
    /// Too few points for this number (how many are on).
    TooFewFor { part: LensPart, have: usize },
    /// The points don't pin it down.
    Unsure { part: LensPart },
    /// It came out at the edge of what a lens can be.
    AtLimit { part: LensPart },
    /// With four points a wrong one can't be told from the others.
    CantTellWhich,
    /// The fit is poor and the shift wasn't solved: it may be wrong.
    ShiftMayBeWrong,
    #[serde(other)]
    Unknown,
}

/// The point that disagrees with the others.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Outlier {
    /// Index into the points.
    #[serde(default)]
    pub point: usize,
    /// How far off it is (pixels) when the others are solved without it.
    #[serde(default)]
    pub off_px: f64,
    /// How well the others agree then (pixels, RMS).
    #[serde(default)]
    pub rest_px: f64,
}

/// A line-up's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Solution {
    #[serde(default)]
    pub pose: Pose,
    #[serde(default)]
    pub lens: LensFit,
    #[serde(default)]
    pub rung: Rung,
    /// Each point's error in pixels, in the points' order; None for one
    /// switched off.
    #[serde(default)]
    pub residuals: Vec<Option<f64>>,
    /// Over the points used (pixels).
    #[serde(default)]
    pub rms_px: f64,
    #[serde(default)]
    pub max_px: f64,
    #[serde(default)]
    pub worst: Option<Outlier>,
    #[serde(default)]
    pub notes: Vec<Note>,
    /// The picture's size the points were in.
    #[serde(default)]
    pub pixels: (u32, u32),
    /// `projector::points_key` of the kept points it was solved from
    /// (`Problem::key`).
    #[serde(default)]
    pub points_key: u64,
    /// The refinement stopped at `model::MAX_ROUNDS` still moving (the
    /// answer is the best it reached; nudging again carries on from it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
}

/// Why nothing was solved.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "why")]
pub(crate) enum Refusal {
    /// Fewer than four points are on.
    TooFew { have: usize },
    /// The picture has no size.
    NoPicture,
    /// A point has a number that isn't one.
    NotANumber { point: usize },
    /// Two points are the same spot on the model.
    SameSpot { a: usize, b: usize },
    /// Two points are at the same place in the picture.
    SamePixel { a: usize, b: usize },
    /// The points are all in one line on the model.
    InALine,
    /// The points are all in one line in the picture.
    InALineInPicture,
    /// Four points on one flat surface, three of them in a line.
    ThreeInALine { a: usize, b: usize, c: usize },
    /// No place for the projector has every point in front of it.
    Behind,
    /// The points fit a picture flipped left to right (a mirror, or the
    /// projector's own rear setting), which no projector placement gives.
    Mirrored,
    /// A point is on a screen that isn't in the stage any more.
    NoScreen { point: usize },
    #[serde(other)]
    Unknown,
}

/// The lens and the pose, the rung's numbers from `s`.
struct Fitted {
    capped: bool,
    state: State,
    rung: Rung,
    cost: f64,
}

/// Solve a line-up (the module's words say how).
pub(crate) fn solve(problem: &Problem) -> Result<Solution, Refusal> {
    let used = check(problem)?;
    let pixels = (problem.pixels.0 as f64, problem.pixels.1 as f64);
    let pairs: Vec<Pair> = used
        .iter()
        .map(|&i| (problem.points[i].world, problem.points[i].pixel))
        .collect();
    let lens = problem.lens.sane();
    let world: Vec<V3> = pairs.iter().map(|p| p.0).collect();
    let (_, spread, _) = linalg::spread(&world);
    let size = spread[2].max(0.0).sqrt().max(1e-3);
    let flat = spread[0].max(0.0).sqrt() < FLAT * spread[2].sqrt();

    let mut notes = Vec::new();
    let mut fit = ladder(problem, &pairs, lens, pixels, size, flat, false, &mut notes)?;
    // The ring of places round the points, only when the quick guesses leave
    // them over a pixel out and nothing better is there to start from: no
    // placement given, or four or five points not on one surface (no linear
    // guess). A nudge, from the last answer, never needs it.
    let linear = flat || pairs.len() >= THROW_POINTS;
    if fit.cost > pairs.len() as f64 && (problem.start.is_none() || !linear) {
        let mut ring_notes = Vec::new();
        if let Ok(f) = ladder(
            problem,
            &pairs,
            lens,
            pixels,
            size,
            flat,
            true,
            &mut ring_notes,
        ) && f.cost < fit.cost
        {
            fit = f;
            notes = ring_notes;
        }
    }

    fit.state.r = linalg::nearest_rotation(&fit.state.r);
    let mut residuals = vec![None; problem.points.len()];
    let mut sum = 0.0;
    let mut max: f64 = 0.0;
    for (k, &i) in used.iter().enumerate() {
        let e = model::project(&fit.state, pixels, pairs[k].0)
            .map(|p| (p[0] - pairs[k].1[0]).hypot(p[1] - pairs[k].1[1]))
            .ok_or(Refusal::Behind)?;
        residuals[i] = Some(e);
        sum += e * e;
        max = max.max(e);
    }
    // Leave one out: the point whose removal leaves the others agreeing best.
    // Not when every point is within a pixel: then none stands out (and a
    // nudge on a good line-up stays quick).
    let mut worst = None;
    if pairs.len() > POSE_POINTS && max >= 1.0 {
        // Only the points furthest off are tried: a point that stands out
        // is among them, and a few dozen points stay quick.
        let mut order: Vec<usize> = (0..pairs.len()).collect();
        let err = |k: usize| residuals[used[k]].unwrap_or(0.0);
        order.sort_by(|&a, &b| err(b).total_cmp(&err(a)));
        let mut best: Option<Outlier> = None;
        for &k in order.iter().take(LEAVE_OUT_TRIES) {
            let rest: Vec<Pair> = pairs
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != k)
                .map(|(_, p)| *p)
                .collect();
            let rung = rung_for(&fit.rung, rest.len(), flat);
            let Some((s, c, _)) = model::refine(
                &fit.state,
                &rung,
                pixels,
                &rest,
                size,
                model::LEAVE_OUT_ROUNDS,
            ) else {
                continue;
            };
            let rest_px = (c / rest.len() as f64).sqrt();
            let off_px = model::project(&s, pixels, pairs[k].0)
                .map(|p| (p[0] - pairs[k].1[0]).hypot(p[1] - pairs[k].1[1]))
                .unwrap_or(f64::INFINITY);
            if best.is_none_or(|b| rest_px < b.rest_px) {
                best = Some(Outlier {
                    point: used[k],
                    off_px,
                    rest_px,
                });
            }
        }
        worst = best.filter(|b| {
            b.off_px >= STANDS_OUT_PX && b.off_px >= STANDS_OUT_TIMES * b.rest_px.max(0.05)
        });
    }

    let rms_px = (sum / used.len() as f64).sqrt();
    if rms_px > MIRROR_PX
        && worst.is_none()
        && mirrored(problem, &pairs, lens, pixels, size, flat, rms_px)
    {
        return Err(Refusal::Mirrored);
    }
    if pairs.len() == POSE_POINTS {
        notes.push(Note::CantTellWhich);
    }
    if fit.rung < Rung::Shift && worst.is_none() && rms_px > POOR_PX {
        notes.push(Note::ShiftMayBeWrong);
    }
    Ok(Solution {
        pose: Pose::from_turn(&fit.state.r, fit.state.c),
        lens: fit.state.lens,
        rung: fit.rung,
        residuals,
        rms_px,
        max_px: max,
        worst,
        notes,
        pixels: problem.pixels,
        points_key: problem.key,
        capped: fit.capped,
    })
}

/// Whether the points fit far better flipped left to right (`u → 1 − u`,
/// the shift's x the other way): a mirrored rig, refused in words rather
/// than answered with a poor fit.
fn mirrored(
    problem: &Problem,
    pairs: &[Pair],
    lens: LensFit,
    pixels: (f64, f64),
    size: f64,
    flat: bool,
    rms_px: f64,
) -> bool {
    let flipped: Vec<Pair> = pairs
        .iter()
        .map(|(x, p)| (*x, [pixels.0 - p[0], p[1]]))
        .collect();
    let mut l = lens;
    l.shift[0] = -l.shift[0];
    let mut notes = Vec::new();
    let plain = Problem {
        start: None,
        ..problem.clone()
    };
    let Ok(fit) = ladder(&plain, &flipped, l, pixels, size, flat, false, &mut notes) else {
        return false;
    };
    let rms = (fit.cost / flipped.len() as f64).sqrt();
    rms < POOR_PX && rms * 5.0 < rms_px
}

/// The rung a leave-one-out solves: the one used, or the highest below it
/// that `n` points allow.
fn rung_for(used: &Rung, n: usize, flat: bool) -> Rung {
    let allowed = if flat || n < THROW_POINTS {
        Rung::Pose
    } else if n < SHIFT_POINTS {
        Rung::Throw
    } else {
        Rung::Aspect
    };
    if *used < allowed {
        used.clone()
    } else {
        allowed
    }
}

/// The points that are on, refused in words if they can't give an answer.
fn check(problem: &Problem) -> Result<Vec<usize>, Refusal> {
    let (w, h) = problem.pixels;
    if w == 0 || h == 0 || w > 1 << 16 || h > 1 << 16 {
        return Err(Refusal::NoPicture);
    }
    let used: Vec<usize> = (0..problem.points.len())
        .filter(|&i| problem.points[i].on)
        .collect();
    for &i in &used {
        let p = &problem.points[i];
        if !p.world.iter().chain(&p.pixel).all(|v| v.is_finite()) {
            return Err(Refusal::NotANumber { point: i });
        }
    }
    if used.len() < POSE_POINTS {
        return Err(Refusal::TooFew { have: used.len() });
    }
    for (k, &a) in used.iter().enumerate() {
        for &b in &used[k + 1..] {
            let (pa, pb) = (&problem.points[a], &problem.points[b]);
            if linalg::norm(linalg::sub(pa.world, pb.world)) < SAME_SPOT {
                return Err(Refusal::SameSpot { a, b });
            }
            if (pa.pixel[0] - pb.pixel[0]).hypot(pa.pixel[1] - pb.pixel[1]) < SAME_PIXEL {
                return Err(Refusal::SamePixel { a, b });
            }
        }
    }
    let world: Vec<V3> = used.iter().map(|&i| problem.points[i].world).collect();
    let (_, s, _) = linalg::spread(&world);
    if s[1].max(0.0).sqrt() < IN_A_LINE * s[2].sqrt() {
        return Err(Refusal::InALine);
    }
    let seen: Vec<[f64; 2]> = used.iter().map(|&i| problem.points[i].pixel).collect();
    let (_, s2) = linalg::spread2(&seen);
    if s2[0].max(0.0).sqrt() < IN_A_LINE * s2[1].sqrt() {
        return Err(Refusal::InALineInPicture);
    }
    if used.len() == POSE_POINTS && s[0].max(0.0).sqrt() < FLAT * s[2].sqrt() {
        // Four on one surface: any three in a line leave the turn loose.
        for a in 0..4 {
            for b in a + 1..4 {
                for c in b + 1..4 {
                    let (pa, pb, pc) = (world[a], world[b], world[c]);
                    let u = linalg::sub(pb, pa);
                    let v = linalg::sub(pc, pa);
                    let sin = linalg::norm(linalg::cross(u, v))
                        / (linalg::norm(u) * linalg::norm(v)).max(1e-300);
                    if sin < IN_A_LINE * 10.0 {
                        return Err(Refusal::ThreeInALine {
                            a: used[a],
                            b: used[b],
                            c: used[c],
                        });
                    }
                }
            }
        }
    }
    Ok(used)
}

/// The best pose, then each rung the points and `problem.free` allow.
// The solve's fixed inputs, passed down as they are.
#[allow(clippy::too_many_arguments)]
fn ladder(
    problem: &Problem,
    pairs: &[Pair],
    lens: LensFit,
    pixels: (f64, f64),
    size: f64,
    flat: bool,
    ring: bool,
    notes: &mut Vec<Note>,
) -> Result<Fitted, Refusal> {
    let mut starts: Vec<State> = Vec::new();
    if let Some(p) = problem.start.filter(|p| p.sane()) {
        starts.push(State {
            r: p.turn(),
            c: p.position,
            lens,
        });
    }
    starts.extend(start::homography(pairs, &lens, pixels));
    let dlt = (!flat && pairs.len() >= THROW_POINTS)
        .then(|| start::dlt(pairs, pixels))
        .flatten();
    if let Some(d) = &dlt {
        // Its place and turn, with the lens as entered.
        starts.push(State { lens, ..d.clone() });
    }

    // Each guess a few rounds, the best of them to the end; the ring too
    // when asked for, or when no other guess has the points in front.
    let mut best = best_of(&starts, pixels, pairs, size);
    if ring || best.is_none() {
        let ring = start::ring(pairs, &lens, pixels, 8);
        if let Some(r) = best_of(&ring, pixels, pairs, size)
            && best.as_ref().is_none_or(|b| r.cost < b.cost)
        {
            best = Some(r);
        }
    }
    let best = best.ok_or(Refusal::Behind)?;
    let (state, cost, capped) = model::refine(
        &best.state,
        &Rung::Pose,
        pixels,
        pairs,
        size,
        model::MAX_ROUNDS,
    )
    .ok_or(Refusal::Behind)?;
    let best = Fitted {
        state,
        rung: Rung::Pose,
        cost,
        capped,
    };

    // The highest rung the points and the caller allow, then down a rung
    // at a time until one's numbers are pinned down and inside a lens.
    let free = problem.free;
    let n = pairs.len();
    let wanted = [
        (Rung::Throw, LensPart::Throw, free.throw, THROW_POINTS),
        (Rung::Shift, LensPart::Shift, free.shift, SHIFT_POINTS),
        (Rung::Aspect, LensPart::Aspect, free.aspect, SHIFT_POINTS),
    ];
    if flat && free.throw {
        notes.push(Note::Flat);
        return Ok(best);
    }
    let mut top = 0;
    for &(_, part, asked, need) in &wanted {
        if !asked {
            // A number not freed stays as entered; the ones above it need
            // it, so the ladder stops there.
            break;
        }
        if n < need {
            notes.push(Note::TooFewFor { part, have: n });
            break;
        }
        top += 1;
    }
    for (rung, part, _, _) in wanted[..top].iter().rev().cloned() {
        let mut tries = vec![best.state.clone()];
        if let Some(d) = &dlt {
            // The DLT's lens numbers up to this rung, the rest as entered.
            let mut l = lens;
            l.throw = d.lens.throw;
            if rung >= Rung::Shift {
                l.shift = d.lens.shift;
            }
            if rung >= Rung::Aspect {
                l.aspect = d.lens.aspect;
            }
            tries.push(State {
                lens: l,
                ..d.clone()
            });
        }
        // Each try a few rounds, judged there (the sds hardly move after);
        // only a rung that passes is taken to the end.
        let mut found: Option<Fitted> = None;
        for t in &tries {
            if let Some((state, cost, capped)) =
                model::refine(t, &rung, pixels, pairs, size, model::START_ROUNDS)
                && found.as_ref().is_none_or(|b| cost < b.cost)
            {
                found = Some(Fitted {
                    state,
                    rung: rung.clone(),
                    cost,
                    capped,
                });
            }
        }
        let why = match &found {
            None => Note::Unsure { part },
            Some(f) => match at_limit(&f.state.lens, &rung) {
                Some(part) => Note::AtLimit { part },
                None => match unsure(&f.state, &rung, pixels, pairs, size) {
                    Some(part) => Note::Unsure { part },
                    None => {
                        let (state, cost, capped) =
                            model::refine(&f.state, &rung, pixels, pairs, size, model::MAX_ROUNDS)
                                .unwrap_or((f.state.clone(), f.cost, f.capped));
                        return Ok(Fitted {
                            state,
                            rung,
                            cost,
                            capped,
                        });
                    }
                },
            },
        };
        if !notes.contains(&why) {
            notes.push(why);
        }
    }
    Ok(best)
}

/// The best of `starts` after a few rounds each (pose only), the first of
/// equals.
fn best_of(starts: &[State], pixels: (f64, f64), pairs: &[Pair], size: f64) -> Option<Fitted> {
    let mut best: Option<Fitted> = None;
    for s in starts {
        if let Some((state, cost, _)) =
            model::refine(s, &Rung::Pose, pixels, pairs, size, model::START_ROUNDS)
            && best.as_ref().is_none_or(|b| cost < b.cost)
        {
            best = Some(Fitted {
                state,
                rung: Rung::Pose,
                cost,
                capped: false,
            });
        }
    }
    best
}

/// The first lens number the rung frees that sits on a bound.
fn at_limit(l: &LensFit, rung: &Rung) -> Option<LensPart> {
    let on = |v: f64, lo: f64, hi: f64| v <= lo * (1.0 + 1e-6) || v >= hi * (1.0 - 1e-6);
    let s = MAX_SHIFT as f64;
    if on(l.throw, MIN_THROW as f64, MAX_THROW as f64) {
        return Some(LensPart::Throw);
    }
    if *rung >= Rung::Shift && l.shift.iter().any(|v| v.abs() >= s * (1.0 - 1e-6)) {
        return Some(LensPart::Shift);
    }
    if *rung >= Rung::Aspect && on(l.aspect, MIN_ASPECT, MAX_ASPECT) {
        return Some(LensPart::Aspect);
    }
    None
}

/// The first lens number the rung frees that the points don't pin down.
fn unsure(
    s: &State,
    rung: &Rung,
    pixels: (f64, f64),
    pairs: &[Pair],
    size: f64,
) -> Option<LensPart> {
    let Some(sd) = model::spreads(s, rung, pixels, pairs, size) else {
        return Some(match rung {
            Rung::Throw => LensPart::Throw,
            Rung::Shift => LensPart::Shift,
            _ => LensPart::Aspect,
        });
    };
    if sd[6] > SURE * s.lens.throw {
        return Some(LensPart::Throw);
    }
    if *rung >= Rung::Shift && (sd[7] > SURE || sd[8] > SURE) {
        return Some(LensPart::Shift);
    }
    if *rung >= Rung::Aspect && sd[9] > SURE * s.lens.aspect {
        return Some(LensPart::Aspect);
    }
    None
}

/// Pixels in words: a tenth below ten, whole above.
fn px(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

impl Solution {
    /// Where the projector is, in the show's numbers.
    pub fn placement(&self) -> Placement {
        self.pose.placement()
    }

    /// `lens` with what this solve worked out put in (the rest as it was).
    /// `pixels` is the picture's size, to tell whether a solved shape is the
    /// pixels' own (then no `aspect` is kept).
    pub fn lens_onto(&self, lens: &Lens, pixels: (u32, u32)) -> Lens {
        let mut out = lens.clone();
        // (A later NewQ's rung is above or below none: nothing is put in.)
        if self.rung >= Rung::Throw {
            out.throw = self.lens.throw as f32;
        }
        if self.rung >= Rung::Shift {
            out.shift = self.lens.shift.map(|v| v as f32);
        }
        if self.rung == Rung::Aspect {
            let own = pixels.0.max(1) as f64 / pixels.1.max(1) as f64;
            out.aspect =
                ((self.lens.aspect - own).abs() > own * 1e-4).then_some(self.lens.aspect as f32);
        }
        out
    }

    /// The answer as the show keeps it (`LineUp::solved`); the placement and
    /// lens it found go on the projector itself.
    pub fn solved(&self) -> Solved {
        Solved {
            rung: self.rung.clone(),
            rms_px: self.rms_px as f32,
            max_px: self.max_px as f32,
            pixels: self.pixels,
            points_key: self.points_key,
            extra: serde_json::Map::new(),
        }
    }

    /// The fit, and what was worked out, in words for the operator.
    pub fn words(&self) -> String {
        let mut s: Vec<String> = Vec::new();
        if let Some(w) = self.worst {
            s.push(format!(
                "Point {} is {} px off: is it the right spot? Without it the others agree to \
                 within {} px; switch it off to use them alone.",
                w.point + 1,
                px(w.off_px),
                px(w.rest_px.max(0.05)),
            ));
        } else if self.max_px < HALF {
            s.push("Lined up to within half a pixel.".into());
        } else {
            s.push(format!(
                "Lined up to within about {} pixels.",
                px(self.max_px)
            ));
        }
        let l = &self.lens;
        let shift = || {
            let side = |v: f64, plus: &str, minus: &str| {
                format!(
                    "{:.0}% {}",
                    (v * 100.0).abs(),
                    if v < 0.0 { minus } else { plus }
                )
            };
            format!(
                "{}, {}",
                side(l.shift[0], "right", "left"),
                side(l.shift[1], "up", "down")
            )
        };
        s.push(match self.rung {
            Rung::Pose | Rung::Unknown(_) => {
                format!("Using the lens you entered (throw {:.2}).", l.throw)
            }
            Rung::Throw => format!("Worked out the throw from the points: {:.2}.", l.throw),
            Rung::Shift => format!(
                "Worked out the throw ({:.2}) and the lens shift ({}) from the points.",
                l.throw,
                shift()
            ),
            Rung::Aspect => format!(
                "Worked out the throw ({:.2}), the lens shift ({}) and the picture's shape \
                 ({:.2}:1) from the points.",
                l.throw,
                shift(),
                l.aspect
            ),
        });
        for n in &self.notes {
            if let Some(w) = n.words() {
                s.push(w);
            }
        }
        s.join(" ")
    }
}

impl Note {
    fn words(&self) -> Option<String> {
        Some(match self {
            Note::Flat => "These points are all on one flat surface, so the lens can't be worked \
                           out from them."
                .into(),
            Note::TooFewFor { part, have } => {
                let need = match part {
                    LensPart::Throw => THROW_POINTS,
                    _ => SHIFT_POINTS,
                };
                format!(
                    "With {need} points or more (there are {have}), not all on one flat \
                     surface, NewQ can work out the {} too.",
                    part.words()
                )
            }
            Note::Unsure { part } => format!(
                "The points don't pin the {} down well enough, so it is as you entered it: \
                 spread them wider, nearer and further from the projector.",
                part.words()
            ),
            Note::AtLimit { part } => format!(
                "The {} came out beyond what a lens can be, so it is as you entered it: check \
                 the points.",
                part.words()
            ),
            Note::CantTellWhich => "With four points a point in the wrong place can't be told \
                                    from the others: add another."
                .into(),
            Note::ShiftMayBeWrong => "If the lens is shifted, enter roughly how much: a wrong \
                                      shift moves where the projector seems to be."
                .into(),
            Note::Unknown => return None,
        })
    }
}

impl Refusal {
    /// Why, in words for the operator.
    pub fn words(&self) -> String {
        match self {
            Refusal::TooFew { have } => format!(
                "Line-up needs at least four points; there {} {have}.",
                if *have == 1 { "is" } else { "are" }
            ),
            Refusal::NoPicture => "The projector's picture has no size yet.".into(),
            Refusal::NotANumber { point } => {
                format!(
                    "Point {} has a number that isn't one: place it again.",
                    point + 1
                )
            }
            Refusal::SameSpot { a, b } => format!(
                "Points {} and {} are the same spot on the model: move one, or switch it off.",
                a + 1,
                b + 1
            ),
            Refusal::SamePixel { a, b } => format!(
                "Points {} and {} are at the same place in the picture: move one, or switch it \
                 off.",
                a + 1,
                b + 1
            ),
            Refusal::InALine => "The points are all in one line on the model, so the \
                                 projector could be anywhere around it: add a point off the line."
                .into(),
            Refusal::InALineInPicture => "The points are all in one line in the picture: spread \
                                          them across it."
                .into(),
            Refusal::ThreeInALine { a, b, c } => format!(
                "Points {}, {} and {} are in a line, so four points on one surface aren't \
                 enough: add another, away from that line.",
                a + 1,
                b + 1,
                c + 1
            ),
            Refusal::Mirrored => "The points fit the picture flipped left to right: is the \
                                  projector going through a mirror, or set to rear projection? \
                                  Line it up with its picture the right way round."
                .into(),
            Refusal::Behind => "No place for the projector has all the points in front of it: \
                                check each point is matched to the right spot."
                .into(),
            Refusal::NoScreen { point } => format!(
                "Point {} is on a screen that isn't in the 3D stage any more: place it again, \
                 or switch it off.",
                point + 1
            ),
            Refusal::Unknown => "This line-up can't be solved here.".into(),
        }
    }
}

/// Where `world` (room metres) is in the picture of a projector at
/// `placement` with `lens`, the picture `pixels` big: pixels from its
/// top-left, None behind the lens. The very sum the solver fits (in f64),
/// so an overlay drawn with it and the solver's errors agree.
pub(crate) fn project(
    placement: &Placement,
    lens: &Lens,
    pixels: (u32, u32),
    world: [f32; 3],
) -> Option<[f32; 2]> {
    let s = model::state_of(placement, lens, pixels);
    let p = model::project(
        &s,
        (pixels.0.max(1) as f64, pixels.1.max(1) as f64),
        world.map(|v| v as f64),
    )?;
    Some(p.map(|v| v as f32))
}

/// A rigid fit: the turn and the move that best take each `from[i]` onto
/// `to[i]` (least squares, no scale, never a mirror), and the RMS distance
/// left (metres). `rotation` is row-major and acts on column vectors:
/// `to ≈ rotation · from + translation`. None for fewer than three pairs,
/// lengths that differ, numbers that aren't, or points all in one line.
/// (For lining up a screen instead of a projector, and a tracker's frame.)
pub(crate) fn fit_rigid(from: &[[f64; 3]], to: &[[f64; 3]]) -> Option<(M3, V3, f64)> {
    if from.len() != to.len()
        || from.len() < 3
        || !from.iter().chain(to).flatten().all(|v| v.is_finite())
    {
        return None;
    }
    let (cf, sf, _) = linalg::spread(from);
    let (ct, st, _) = linalg::spread(to);
    for s in [sf, st] {
        if s[1].max(0.0).sqrt() < IN_A_LINE * s[2].max(0.0).sqrt() || s[2] <= 0.0 {
            return None;
        }
    }
    let mut m = [[0.0; 3]; 3];
    for (a, b) in from.iter().zip(to) {
        let a = linalg::sub(*a, cf);
        let b = linalg::sub(*b, ct);
        for (r, row) in m.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v += b[r] * a[k];
            }
        }
    }
    let rotation = linalg::nearest_rotation(&m);
    let translation = linalg::sub(ct, linalg::mul_v(&rotation, cf));
    let sum: f64 = from
        .iter()
        .zip(to)
        .map(|(a, b)| {
            let d = linalg::sub(linalg::add(linalg::mul_v(&rotation, *a), translation), *b);
            linalg::dot(d, d)
        })
        .sum();
    Some((rotation, translation, (sum / from.len() as f64).sqrt()))
}
