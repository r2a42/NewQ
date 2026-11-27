//! Line-up on the 3D stage's page (mapping plan 4.4, old plan 5.2): the
//! points in progress (`LineUpDraft`, `UiState::line_up`), never in the show
//! until *Done*.
//!
//! The app (`app.rs`, `net::lineup::Sender`) reads the draft each pass: while
//! there is one it sends `draft.overlay(..)` to the computer that drives the
//! projector's display (this one alone: one path), and sends `End` when the
//! draft is gone (*Done* or *Cancel*) or its session changes. It writes back
//! how the link stands (`link`, `link_words`), never saved. *Done* is one
//! ordinary edit of the show (placement, lens, `LineUp`), sent by show sync.
//!
//! The flow (old plan 5.2), from a projector's row on Settings ▸ 3D stage ▸
//! Projectors (*Line up…*; a projector with a display asks first, since its
//! display shows black and the crosses meanwhile):
//! - The page becomes two panes. **Left, the model** through the
//!   projector's lens (as it stands in the show; drag to look round, the
//!   wheel to come closer, double-click back to the lens). A click on a
//!   screen drops a numbered point there, snapped to the nearest corner of
//!   the screen (or of a facet) within `SNAP_PT`; with Alt held it goes
//!   where clicked. A click near a point picks it.
//! - **Right, the projector's picture**, its pixels' shape, with the same
//!   numbers: drag one, or click it and use the arrow keys (one pixel; Shift
//!   ten; Alt a tenth) while looking at the wall. The picture pane holds the
//!   keyboard while a point is picked (as a text box would: Space is no GO
//!   meanwhile; Esc still fades everything out). The model's outline as the
//!   present solve throws it is drawn there too.
//! - **Under them**, the solve in words (`calib::Solution::words` or
//!   `Refusal::words`), each point's error, and *Done*, *Cancel* and *Back
//!   to before line-up*.
//! - The solve is made again only when what it is made from changes (the
//!   points, the picture's size, the projector as entered, the screens the
//!   points are on), never per frame, from the last answer (`Problem::start`)
//!   with the lens as entered.
//! - **Mirrored on one flat screen** can't be seen by the solver (a flipped
//!   picture fits from behind the screen), so with every point on one flat
//!   screen the answer must be on the side it is lit from (`Show::kind_of`):
//!   else it is `Refusal::Mirrored`'s words and no *Done*.
//! - **The draft file**: the draft is written to `DRAFT_FILE` in the node
//!   folder whenever it changes (with the show it is for), and taken away by
//!   *Done*, *Cancel* and *Back to before line-up*. A restart offers "Carry
//!   on lining up Projector 2?" on the page, with a new session (the crosses
//!   of the old one are passed by).
//! - **Refused** in Show mode, on a follower (its show is run from the
//!   primary) and while Test Mode runs, in words; a draft is put away (kept in
//!   its file, the crosses ended) when any of those starts, its projector
//!   goes or another show is opened (`tend`, which the page calls and the
//!   app is to call each pass).

use std::collections::hash_map::DefaultHasher;
use std::hash::{BuildHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Pos2, Rect, RichText, Sense, Ui, Vec2, vec2};

use super::UiState;
use super::settings::{hint, warning};
use crate::calib::{self, Pose, Problem, Refusal, Solution};
use crate::gpu::Slot;
use crate::net::lineup::{Link, Mark, Overlay, Under};
use crate::preview_pace::PreviewId;
use crate::render::{OutputGroup, OutputLayer, OutputPreview};
use crate::render3d::{self, Camera, Orbit};
use crate::show::{OutputId, Show};
use crate::stage3d::projector::{Before, LineUp, LinePoint, Projector, ProjectorId};
use crate::stage3d::{ScreenKind, Stage3d, SurfaceId, SurfaceShape, geometry};
use serde::{Deserialize, Serialize};

/// A line-up in progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineUpDraft {
    pub projector: ProjectorId,
    /// A random number for this line-up (`Overlay::session`): a new one for
    /// each start, so the crosses of an earlier one are passed by.
    #[serde(default)]
    pub session: u64,
    /// The points so far, as the show keeps them (`at` in a screen's own
    /// frame when on one).
    #[serde(default)]
    pub points: Vec<LinePoint>,
    /// The picture's size the points are placed in.
    #[serde(default)]
    pub pixels: (u32, u32),
    /// The point being moved (an index into `points`).
    #[serde(default)]
    pub active: Option<usize>,
    /// What is under the crosses on the projector.
    #[serde(default)]
    pub under: Under,
    /// Where the projector stood, and its lens, when line-up began.
    #[serde(default)]
    pub before: Option<Before>,
    /// The model's outline as the present solve throws it, in `pixels`
    /// (`calib::project`); None before a solve. Set by the panes at each
    /// solve, with `outline_rev` counted up.
    #[serde(skip)]
    pub outline: Option<Vec<Vec<[f32; 2]>>>,
    #[serde(skip)]
    pub outline_rev: u64,
    /// How the link to the crosses stands (the app, each pass).
    #[serde(skip)]
    pub link: Option<Link>,
    /// It in words, when there is something to say (`lineup::link_words`).
    #[serde(skip)]
    pub link_words: Option<String>,
}

impl LineUpDraft {
    pub fn new(projector: ProjectorId, pixels: (u32, u32), session: u64) -> LineUpDraft {
        LineUpDraft {
            projector,
            session,
            points: Vec::new(),
            pixels,
            active: None,
            under: Under::Black,
            before: None,
            outline: None,
            outline_rev: 0,
            link: None,
            link_words: None,
        }
    }

    /// What the crosses show now, for `display` (the session and count are
    /// the sender's; the outline goes separately): a mark per point, at its
    /// pixel, numbered from 1.
    // (Sent by the app: round 2, agent B.)
    #[allow(dead_code)]
    pub fn overlay(&self, display: OutputId) -> Overlay {
        Overlay {
            session: self.session,
            seq: 0,
            display,
            pixels: self.pixels,
            marks: self
                .points
                .iter()
                .enumerate()
                .map(|(i, p)| Mark {
                    n: (i + 1).min(u16::MAX as usize) as u16,
                    at: p.pixel,
                    on: p.on,
                })
                .collect(),
            active: self
                .active
                .filter(|a| *a < self.points.len())
                .and_then(|a| u8::try_from(a).ok()),
            outline: None,
            under: self.under,
        }
    }
}

/// The left pane's 3D view (governed as every preview is).
pub const LINEUP_VIEW: PreviewId = PreviewId::Other("stage-lineup", 0);

/// The draft file, in the node folder.
pub const DRAFT_FILE: &str = "lineup-draft.json";

/// A click this near a corner (points of the pane) is put on it.
pub const SNAP_PT: f32 = 10.0;
/// A click or press this near a point picks it.
const PICK_PT: f32 = 14.0;
/// The arrow keys' steps (pixels of the picture): plain, Shift, Alt.
pub const STEP: f32 = 1.0;
pub const STEP_SHIFT: f32 = 10.0;
pub const STEP_ALT: f32 = 0.1;

/// The button on a projector's row.
pub const LINE_UP: &str = "Line up…";
pub const DONE: &str = "Done";
pub const CANCEL: &str = "Cancel";
pub const BACK: &str = "Back to before line-up";
pub const CARRY_ON: &str = "Carry on";
pub const FORGET: &str = "Forget it";
pub const BLANK_YES: &str = "Line it up";
pub const BLANK_NO: &str = "Not now";
pub const DIMMED: &str = "Its picture, dimmed, under the crosses";

/// Why line-up can't start or go on here, in words.
pub const SHOW_MODE_WORDS: &str = "Line-up isn't possible in Show mode: leave Show mode first.";
pub const LOCKED_WORDS: &str =
    "This computer follows a primary, so its show is run from there: line projectors up on the \
     primary.";
pub const TEST_WORDS: &str = "Line-up waits while Test Mode runs.";

/// How to work the panes.
pub const PANES_HINT: &str = "Left: click a spot on the model you can find in the room (it snaps \
     to a corner; hold Alt to place it freely). Right: drag its number, or click it and use the \
     arrow keys (Shift 10 pixels, Alt a tenth), until the cross on the wall sits on that spot.";

/// What the start asks for a projector whose display shows something.
pub fn blanks_words(name: &str) -> String {
    format!(
        "This blanks {name} while you line it up: its display shows black and the crosses \
         instead of its picture."
    )
}

/// The draft's file: the draft and the show it is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftFile {
    /// The show's file (None for one not saved yet) and name.
    #[serde(default)]
    pub show: Option<PathBuf>,
    #[serde(default)]
    pub show_name: String,
    pub draft: LineUpDraft,
}

/// The last solve.
#[derive(Clone, Debug)]
struct Solve {
    key: u64,
    result: Result<Solution, Refusal>,
}

/// The panes' own state (`ProjectorsUi::lineup`): what isn't the draft.
#[derive(Default)]
pub struct Panes {
    /// Where the draft is kept: the node folder unless set (tests set it).
    pub draft_dir: Option<PathBuf>,
    /// The left view, once moved from the lens.
    orbit: Option<Orbit>,
    solve: Option<Solve>,
    /// How many solves have been made (the tests count them).
    pub solves: u64,
    /// The draft file as last written.
    written: Option<String>,
    /// Whether the draft file has been looked for, and the draft it offers.
    looked: bool,
    pub offer: Option<LineUpDraft>,
    /// The show the draft is for (its file, its name).
    show: Option<(Option<PathBuf>, String)>,
    /// A projector whose start asks first (it blanks its display).
    pub asking: Option<ProjectorId>,
    /// A point being dragged, and how far its middle is from the pointer.
    drag: Option<(usize, Vec2)>,
    /// Put the keyboard on the picture pane next frame.
    want_focus: bool,
    /// Something to say on the page (a draft put away, a start refused).
    pub note: Option<String>,
}

impl Panes {
    fn dir(&self) -> PathBuf {
        self.draft_dir
            .clone()
            .unwrap_or_else(crate::net::node::node_dir)
    }

    fn file(&self) -> PathBuf {
        self.dir().join(DRAFT_FILE)
    }

    /// The draft as the file has it, if it is for show `show`.
    fn read(&self, show: &Show, st_show: &Option<PathBuf>) -> Option<LineUpDraft> {
        let text = std::fs::read_to_string(self.file()).ok()?;
        let f: DraftFile = match serde_json::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("{} isn't readable ({e}); left alone", self.file().display());
                return None;
            }
        };
        (f.show == *st_show && f.show_name == show.name && show.projector(f.draft.projector).is_some())
            .then_some(f.draft)
    }

    /// Write `draft` when it differs from what was last written.
    fn write(&mut self, draft: &LineUpDraft) {
        let Some((file, name)) = self.show.clone() else {
            return;
        };
        let f = DraftFile {
            show: file,
            show_name: name,
            draft: draft.clone(),
        };
        let Ok(text) = serde_json::to_string_pretty(&f) else {
            return;
        };
        if self.written.as_ref() == Some(&text) {
            return;
        }
        let dir = self.dir();
        let tmp = dir.join(format!("{DRAFT_FILE}.tmp"));
        let done = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&tmp, &text))
            .and_then(|_| std::fs::rename(&tmp, self.file()));
        match done {
            Ok(()) => self.written = Some(text),
            Err(e) => {
                log::warn!("The line-up draft wasn't kept: {e}");
                // Not tried again until it changes.
                self.written = Some(text);
            }
        }
    }

    /// The draft file goes (the line-up is over).
    fn forget_file(&mut self) {
        let _ = std::fs::remove_file(self.file());
        self.written = None;
        self.offer = None;
        self.looked = true;
    }
}

/// A new line-up's session: a random number, never 0.
fn new_session() -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    std::process::id().hash(&mut h);
    h.finish() | 1
}

/// Why line-up can't start or go on now, in words; None when it can.
pub fn refused(st: &UiState) -> Option<&'static str> {
    if st.show_mode {
        Some(SHOW_MODE_WORDS)
    } else if super::network::locked(st) {
        Some(LOCKED_WORDS)
    } else if st.test_locked {
        Some(TEST_WORDS)
    } else {
        None
    }
}

/// Each pass (the page calls it; the app is to call it too, so it holds
/// when the page isn't drawn): a draft that may not go on is put away (kept
/// in its file and offered again, its crosses ended): in Show mode, on a
/// follower, in Test Mode, when its projector is gone or another show is
/// open. Whether one was put away.
pub fn tend(show: &Show, st: &mut UiState) -> bool {
    let Some(draft) = &st.line_up else {
        return false;
    };
    let id = draft.projector;
    let gone = show.projector(id).is_none();
    let here = (st.show_path.clone(), show.name.clone());
    let other_show = st
        .stage
        .projectors
        .lineup
        .show
        .as_ref()
        .is_some_and(|s| *s != here);
    let why = refused(st);
    if why.is_none() && !gone && !other_show {
        return false;
    }
    let draft = st.line_up.take();
    let panes = &mut st.stage.projectors.lineup;
    panes.drag = None;
    panes.solve = None;
    panes.note = Some(match (why, gone || other_show) {
        (_, true) => "The line-up in progress was put away: its projector isn't in this show."
            .to_string(),
        (Some(w), _) => format!(
            "The line-up in progress was put away. {w} It can be carried on afterwards from {}.",
            super::settings::STAGE_PROJECTORS.path()
        ),
        (None, false) => String::new(),
    });
    if !(gone || other_show) {
        // Offered again once line-up may go on.
        panes.offer = draft;
        panes.looked = true;
    } else {
        panes.looked = false;
    }
    true
}

/// Start lining up projector `id` (its stored points carried on, if any).
pub fn start(show: &Show, st: &mut UiState, id: ProjectorId) {
    if let Some(why) = refused(st) {
        st.stage.projectors.lineup.note = Some(why.to_string());
        return;
    }
    let Some(p) = show.projector(id) else {
        return;
    };
    let (pixels, _) = super::stage_projectors::shape_of(show, st, p);
    let mut draft = LineUpDraft::new(id, pixels, new_session());
    let mut note = None;
    match &p.line_up {
        Some(lu) => {
            draft.points = lu.points.clone();
            let was = lu.pixels;
            if was != pixels && was.0 > 0 && was.1 > 0 {
                let sx = pixels.0 as f32 / was.0 as f32;
                let sy = pixels.1 as f32 / was.1 as f32;
                for pt in &mut draft.points {
                    pt.pixel = [pt.pixel[0] * sx, pt.pixel[1] * sy];
                }
                note = Some(format!(
                    "The projector's picture was {} × {} when it was last lined up and is {} × {} \
                     now: the points were scaled to it. Check each one.",
                    was.0, was.1, pixels.0, pixels.1
                ));
            }
            draft.before = Some(lu.before.clone().unwrap_or(Before {
                placement: p.placement,
                lens: p.lens.clone(),
                extra: Default::default(),
            }));
        }
        None => {
            draft.before = Some(Before {
                placement: p.placement,
                lens: p.lens.clone(),
                extra: Default::default(),
            });
        }
    }
    draft.active = (!draft.points.is_empty()).then_some(0);
    begin(show, st, draft);
    st.stage.projectors.lineup.note = note;
}

/// `draft` becomes the line-up in progress.
fn begin(show: &Show, st: &mut UiState, draft: LineUpDraft) {
    let panes = &mut st.stage.projectors.lineup;
    panes.show = Some((st.show_path.clone(), show.name.clone()));
    panes.orbit = None;
    panes.solve = None;
    panes.drag = None;
    panes.asking = None;
    panes.offer = None;
    panes.looked = true;
    panes.note = None;
    panes.write(&draft);
    st.stage.projectors.picked = Some(draft.projector);
    st.line_up = Some(draft);
}

/// The panes' key for a solve: everything it is made from.
fn solve_key(show: &Show, draft: &LineUpDraft, p: &Projector) -> u64 {
    let mut h = DefaultHasher::new();
    crate::stage3d::projector::points_key(&draft.points, draft.pixels).hash(&mut h);
    format!("{:?}", (p.placement, &p.lens, p.resolution)).hash(&mut h);
    let mut screens: Vec<SurfaceId> = draft.points.iter().filter_map(|p| p.surface).collect();
    screens.sort();
    screens.dedup();
    for s in screens {
        match show.stage3d.surface(s) {
            Some(s) => format!("{:?}", (s.placement, &s.shape, s.rear, &s.kind)).hash(&mut h),
            None => 0u8.hash(&mut h),
        }
    }
    h.finish()
}

/// The line-up as it would be kept from `draft`'s points.
fn kept(draft: &LineUpDraft) -> LineUp {
    LineUp {
        points: draft.points.clone(),
        pixels: draft.pixels,
        ..LineUp::default()
    }
}

/// Solve `draft` again if what it is made from changed; the outline then
/// goes with it.
fn solve_if_due(show: &Show, panes: &mut Panes, draft: &mut LineUpDraft) {
    let Some(p) = show.projector(draft.projector) else {
        return;
    };
    let key = solve_key(show, draft, p);
    if panes.solve.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let last = panes
        .solve
        .as_ref()
        .and_then(|s| s.result.as_ref().ok())
        .map(|s| s.pose);
    let result = solve(show, p, draft, last);
    panes.solves += 1;
    let outline = match &result {
        Ok(sol) => outline(
            &show.stage3d,
            &sol.placement(),
            &sol.lens_onto(&p.lens, draft.pixels),
            draft.pixels,
        ),
        // Nothing to throw: the crosses alone.
        Err(_) => Vec::new(),
    };
    if draft.outline.as_ref() != Some(&outline) {
        draft.outline = Some(outline);
        draft.outline_rev += 1;
    }
    panes.solve = Some(Solve { key, result });
}

/// The solve of `draft`'s points for projector `p`, from `last` (the last
/// answer) when there is one; refused as mirrored when every point is on one
/// flat screen and the answer is on the wrong side of it.
fn solve(
    show: &Show,
    p: &Projector,
    draft: &LineUpDraft,
    last: Option<Pose>,
) -> Result<Solution, Refusal> {
    let mut problem = Problem::for_line_up(p, &kept(draft), &show.stage3d)?;
    if last.is_some() {
        problem.start = last;
    }
    let sol = calib::solve(&problem)?;
    if wrong_side(show, draft, &sol) {
        return Err(Refusal::Mirrored);
    }
    Ok(sol)
}

/// Whether every point that is on is on one flat screen and the answer has
/// the projector on the side the screen isn't lit from (behind a front
/// screen, in front of a rear one): what a picture flipped left to right
/// fits (the solver can't tell on a flat set).
fn wrong_side(show: &Show, draft: &LineUpDraft, sol: &Solution) -> bool {
    let mut on = draft.points.iter().filter(|p| p.on);
    let Some(first) = on.next().and_then(|p| p.surface) else {
        return false;
    };
    if !on.all(|p| p.surface == Some(first)) {
        return false;
    }
    let Some(s) = show.stage3d.surface(first) else {
        return false;
    };
    if !matches!(s.shape, SurfaceShape::Flat { .. }) {
        return false;
    }
    let Some(local) = calib::to_screen(&show.stage3d, Some(first), sol.pose.position) else {
        return false;
    };
    let in_front = local[2] > 0.0;
    match show.kind_of(s) {
        ScreenKind::Rear => in_front,
        ScreenKind::Front | ScreenKind::Led => !in_front,
    }
}

/// The corners of screen `s` in its own frame: each piece's mesh's corners
/// (a flat screen's four, a curved one's edge, each facet's), a corner two
/// facets share once.
pub fn corners(shape: &SurfaceShape) -> Vec<[f32; 3]> {
    let mut out: Vec<[f32; 3]> = Vec::new();
    for piece in geometry::surface_mesh(shape) {
        for v in &piece.mesh.positions {
            let c = geometry::point(&piece.transform, *v);
            let near = |o: &[f32; 3]| (0..3).all(|k| (o[k] - c[k]).abs() < 1e-3);
            if !out.iter().any(near) {
                out.push(c);
            }
        }
    }
    out
}

/// The edges of a mesh that only one triangle has (its outline), joined
/// into runs of corners.
fn boundary(indices: &[u32]) -> Vec<Vec<u32>> {
    let mut count: std::collections::HashMap<(u32, u32), (u32, (u32, u32))> = Default::default();
    for t in indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let k = (a.min(b), a.max(b));
            count.entry(k).or_insert((0, (a, b))).0 += 1;
        }
    }
    let mut edges: Vec<(u32, u32)> = count
        .into_values()
        .filter(|(n, _)| *n == 1)
        .map(|(_, e)| e)
        .collect();
    edges.sort();
    let mut runs: Vec<Vec<u32>> = Vec::new();
    while let Some((a, b)) = edges.pop() {
        let mut run = vec![a, b];
        loop {
            let end = *run.last().unwrap_or(&a);
            match edges.iter().position(|(x, _)| *x == end) {
                Some(i) => {
                    let (_, y) = edges.swap_remove(i);
                    run.push(y);
                    if y == run[0] {
                        break;
                    }
                }
                None => break,
            }
        }
        runs.push(run);
    }
    runs
}

/// The screens' outlines as a projector at `placement` with `lens` throws
/// them, in pixels of a picture `pixels` big (`calib::project`, the solver's
/// own sum): a piece per run of edges, cut where a corner is behind the
/// lens; at most `lineup::MAX_OUTLINE_PIECES`.
pub fn outline(
    stage: &Stage3d,
    placement: &crate::stage3d::Placement,
    lens: &crate::stage3d::projector::Lens,
    pixels: (u32, u32),
) -> Vec<Vec<[f32; 2]>> {
    let mut pieces: Vec<Vec<[f32; 2]>> = Vec::new();
    for s in stage.surfaces.iter().filter(|s| s.visible) {
        let to_room = s.placement.matrix();
        for piece in geometry::surface_mesh(&s.shape) {
            let m = geometry::mul(&to_room, &piece.transform);
            for run in boundary(&piece.mesh.indices) {
                let mut now: Vec<[f32; 2]> = Vec::new();
                for i in run {
                    let Some(v) = piece.mesh.positions.get(i as usize) else {
                        continue;
                    };
                    match calib::project(placement, lens, pixels, geometry::point(&m, *v)) {
                        Some(px) => now.push(px),
                        None => {
                            if now.len() >= 2 {
                                pieces.push(std::mem::take(&mut now));
                            }
                            now.clear();
                        }
                    }
                }
                if now.len() >= 2 {
                    pieces.push(now);
                }
            }
        }
    }
    pieces.truncate(crate::net::lineup::MAX_OUTLINE_PIECES);
    pieces
}

/// Where a point would be in the picture now: through the last answer, else
/// the projector as it stands; the picture's middle when that is off it.
fn first_pixel(
    show: &Show,
    p: &Projector,
    panes: &Panes,
    draft: &LineUpDraft,
    room: [f32; 3],
) -> [f32; 2] {
    let (w, h) = (draft.pixels.0 as f32, draft.pixels.1 as f32);
    let (placement, lens) = match panes.solve.as_ref().and_then(|s| s.result.as_ref().ok()) {
        Some(sol) => (sol.placement(), sol.lens_onto(&p.lens, draft.pixels)),
        None => (p.placement, p.lens.clone()),
    };
    let _ = show;
    match calib::project(&placement, &lens, draft.pixels, room) {
        // (A corner of a picture that fills its screen is a hair outside
        // by rounding.)
        Some([x, y]) if (-1.0..=w + 1.0).contains(&x) && (-1.0..=h + 1.0).contains(&y) => {
            [x.clamp(0.0, w), y.clamp(0.0, h)]
        }
        _ => [w / 2.0, h / 2.0],
    }
}

/// The page while a line-up is in progress: the two panes, the words and
/// the buttons.
pub(super) fn panes(ui: &mut Ui, show: &mut Show, st: &mut UiState) {
    let Some(mut draft) = st.line_up.take() else {
        return;
    };
    let Some(p) = show.projector(draft.projector).cloned() else {
        st.stage.projectors.lineup.note =
            Some("The projector being lined up isn't in the show any more.".into());
        return;
    };
    let mut panes = std::mem::take(&mut st.stage.projectors.lineup);
    let refusal = refused(st);
    let mut end: Option<End> = None;
    ui.horizontal_wrapped(|ui| {
        ui.heading(format!("Lining up {}", p.name));
        ui.add_space(12.0);
        let ok = panes
            .solve
            .as_ref()
            .is_some_and(|s| s.result.is_ok())
            && refusal.is_none();
        let r = ui
            .add_enabled(ok, egui::Button::new(RichText::new(DONE).strong()))
            .on_hover_text("Put the projector where the points say, and keep the points")
            .on_disabled_hover_text("Nothing is lined up yet: see the words under the panes");
        super::stage_edit::note(ui, "Line-up ▸ Done", &r);
        if r.clicked() {
            end = Some(End::Done);
        }
        let r = ui
            .button(CANCEL)
            .on_hover_text("Leave the projector as it was; the points in progress go");
        super::stage_edit::note(ui, "Line-up ▸ Cancel", &r);
        if r.clicked() {
            end = Some(End::Cancel);
        }
        if draft.before.is_some() {
            let r = ui.button(BACK).on_hover_text(
                "Put the projector back where it stood, with the lens it had, before it was \
                 first lined up",
            );
            super::stage_edit::note(ui, "Line-up ▸ Back", &r);
            if r.clicked() {
                end = Some(End::Back);
            }
        }
        let mut dimmed = draft.under == Under::Dimmed;
        if ui
            .checkbox(&mut dimmed, DIMMED)
            .on_hover_text("Else the projector shows black under the crosses")
            .changed()
        {
            draft.under = if dimmed { Under::Dimmed } else { Under::Black };
        }
    });
    if let Some(why) = refusal {
        warning(ui, why);
    }
    if let Some(w) = &draft.link_words {
        warning(ui, w);
    }
    if let Some(n) = &panes.note {
        hint(ui, n);
    }
    hint(ui, PANES_HINT);
    let room = ui.available_size();
    let h = (room.y - 150.0).clamp(140.0, 520.0);
    let w = ((room.x - 12.0) / 2.0).max(120.0);
    let editable = refusal.is_none();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let mut l = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        model_pane(&mut l, show, st, &mut panes, &mut draft, &p, editable);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let mut r = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        picture_pane(&mut r, &mut panes, &mut draft, editable);
    });
    solve_if_due(show, &mut panes, &mut draft);
    let words = match panes.solve.as_ref().map(|s| &s.result) {
        Some(Ok(sol)) => sol.words(),
        Some(Err(r)) => r.words(),
        None => String::new(),
    };
    let r = ui.label(RichText::new(&words).strong());
    super::stage_edit::note(ui, &format!("Line-up ▸ words ▸ {words}"), &r);
    egui::ScrollArea::vertical()
        .id_salt("lineup_points")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            point_rows(ui, show, &mut panes, &mut draft, editable);
        });
    if !editable {
        panes.drag = None;
    }
    let time = ui.input(|i| i.time);
    match end {
        Some(End::Done) => {
            if let Some(Ok(sol)) = panes.solve.as_ref().map(|s| &s.result) {
                done(show, st, &draft, sol, time);
            }
            panes.forget_file();
            panes.solve = None;
            st.stage.projectors.lineup = panes;
        }
        Some(End::Cancel) => {
            panes.forget_file();
            panes.solve = None;
            st.stage.projectors.lineup = panes;
        }
        Some(End::Back) => {
            if let Some(b) = &draft.before {
                back_to_before(show, st, draft.projector, b, time);
            }
            panes.forget_file();
            panes.solve = None;
            st.stage.projectors.lineup = panes;
        }
        None => {
            panes.write(&draft);
            st.stage.projectors.lineup = panes;
            st.line_up = Some(draft);
        }
    }
}

enum End {
    Done,
    Cancel,
    Back,
}

/// *Done*: the projector where the points say, the points and the answer
/// kept, as one undo step of its own.
fn done(show: &mut Show, st: &mut UiState, draft: &LineUpDraft, sol: &Solution, time: f64) {
    st.history.end_burst();
    let Some(p) = show.projector_mut(draft.projector) else {
        return;
    };
    p.placement = sol.placement();
    p.lens = sol.lens_onto(&p.lens, draft.pixels);
    let mut lu = p.line_up.take().unwrap_or_default();
    lu.points = draft.points.clone();
    lu.pixels = draft.pixels;
    lu.solved = Some(sol.solved());
    // The first line-up's "before" stays.
    if lu.before.is_none() {
        lu.before = draft.before.clone();
    }
    p.line_up = Some(lu);
    one_step(show, st, time);
}

/// *Back to before line-up*: where it stood and its lens before it was
/// first lined up, exactly; the points are kept (not current any more).
fn back_to_before(show: &mut Show, st: &mut UiState, id: ProjectorId, b: &Before, time: f64) {
    st.history.end_burst();
    let Some(p) = show.projector_mut(id) else {
        return;
    };
    p.placement = b.placement;
    p.lens = b.lens.clone();
    if let Some(lu) = &mut p.line_up {
        lu.solved = None;
    }
    one_step(show, st, time);
}

/// The show as it now is is a step of the undo history of its own.
fn one_step(show: &Show, st: &mut UiState, time: f64) {
    if let Ok(json) = serde_json::to_string(show) {
        st.history.observe(&json, time);
    }
    st.history.end_burst();
}

/// The left pane's camera: through the lens as the projector stands in the
/// show, until it is moved.
fn model_camera(show: &Show, st: &UiState, panes: &Panes, p: &Projector) -> Camera {
    match &panes.orbit {
        Some(o) => o.camera(),
        None => {
            let (_, aspect) = super::stage_projectors::shape_of(show, st, p);
            p.camera(aspect)
        }
    }
}

/// The size in whole pixels for camera sums over `rect`.
fn size_of(rect: Rect) -> (u32, u32) {
    (
        rect.width().round().max(1.0) as u32,
        rect.height().round().max(1.0) as u32,
    )
}

/// Where a room point is in the left pane, if in front of its camera.
pub(super) fn model_pos(camera: &Camera, rect: Rect, room: [f32; 3]) -> Option<Pos2> {
    camera
        .project(room, size_of(rect))
        .map(|[x, y]| rect.min + vec2(x, y))
}

/// The left pane: the model through the lens, the points on it, and a
/// click to drop one.
fn model_pane(
    ui: &mut Ui,
    show: &Show,
    st: &mut UiState,
    panes: &mut Panes,
    draft: &mut LineUpDraft,
    p: &Projector,
    editable: bool,
) {
    let rect = ui.max_rect();
    let response = ui.interact(rect, egui::Id::new("lineup_model"), Sense::click_and_drag());
    super::stage_edit::note(ui, "Line-up ▸ model", &response);
    st.stage.in_sight.push(LINEUP_VIEW);
    ui.ctx()
        .request_repaint_after(crate::stage_live::IN_SIGHT_EVERY);
    let ppp = ui.ctx().pixels_per_point();
    // The mouse: look round, come closer, back to the lens.
    // (The wheel's own turn, not what is left of a page's smooth scroll.)
    let scroll: f32 = ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { delta, unit, .. } => match unit {
                    egui::MouseWheelUnit::Point => delta.y,
                    egui::MouseWheelUnit::Line => delta.y * 40.0,
                    egui::MouseWheelUnit::Page => delta.y * 400.0,
                },
                _ => 0.0,
            })
            .sum()
    });
    if response.double_clicked() {
        panes.orbit = None;
    } else if response.dragged() || (response.hovered() && scroll != 0.0) {
        let mut o = panes
            .orbit
            .take()
            .unwrap_or_else(|| Orbit::from_camera(&model_camera(show, st, &Panes::default(), p)));
        let d = response.drag_delta() * ppp;
        if response.dragged_by(egui::PointerButton::Primary) {
            o.orbit(d.x, d.y);
        } else if response.dragged() {
            o.pan(d.x, d.y, rect.height() * ppp);
        }
        if response.hovered() && scroll != 0.0 {
            o.dolly(scroll / 50.0);
        }
        panes.orbit = Some(o);
    }
    let camera = model_camera(show, st, panes, p);
    // A click drops a point, or picks the one it is near.
    if editable
        && response.clicked()
        && !response.double_clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        let free = ui.input(|i| i.modifiers.alt);
        click_model(show, st, panes, draft, p, &camera, rect, at, free);
    }
    // Its picture, when the governor lets it be drawn.
    let budget = st.preview_budget(LINEUP_VIEW);
    let has = st.stage.targets.contains_key(&LINEUP_VIEW);
    if super::stage_view::wants_picture(budget, has) {
        let mut view = super::stage_view::StageViewState::default();
        view.grid = false;
        st.stage.requests.push(super::stage_view::ViewRequest {
            id: LINEUP_VIEW,
            size: super::stage_view::request_size(rect.size() * ppp),
            camera: camera.clone(),
            options: super::stage_view::view_options(&view, budget.scale, st.stage.bounds.as_ref()),
            lines: Arc::new(Vec::new()),
        });
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(15, 16, 19));
    if let Some(t) = st.stage.targets.get(&LINEUP_VIEW) {
        let shown = super::preview::fit_picture((t.width, t.height), rect);
        painter.add(super::preview::callback(
            shown,
            OutputPreview {
                groups: vec![picture_group(t)],
                slot: Slot::Preview,
                pace: None,
            },
        ));
    }
    if panes.orbit.is_none() {
        // The lens's own picture framed; what is outside it shaded.
        let [l, t, r, b] = camera.picture_in(size_of(rect));
        let frame = Rect::from_min_max(rect.min + vec2(l, t), rect.min + vec2(r, b));
        let shade = Color32::from_black_alpha(170);
        for band in [
            Rect::from_min_max(rect.min, egui::pos2(rect.max.x, frame.min.y)),
            Rect::from_min_max(egui::pos2(rect.min.x, frame.max.y), rect.max),
            Rect::from_min_max(
                egui::pos2(rect.min.x, frame.min.y),
                egui::pos2(frame.min.x, frame.max.y),
            ),
            Rect::from_min_max(
                egui::pos2(frame.max.x, frame.min.y),
                egui::pos2(rect.max.x, frame.max.y),
            ),
        ] {
            if band.width() > 0.5 && band.height() > 0.5 {
                painter.rect_filled(band, 0.0, shade);
            }
        }
        painter.rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(1.0, Color32::from_rgb(255, 210, 90)),
            egui::StrokeKind::Inside,
        );
    }
    // The corners a click snaps to.
    for s in show.stage3d.surfaces.iter().filter(|s| s.visible) {
        let m = s.placement.matrix();
        for c in corners(&s.shape) {
            if let Some(at) = model_pos(&camera, rect, geometry::point(&m, c))
                && rect.contains(at)
            {
                painter.circle_filled(at, 2.5, Color32::from_rgb(0, 200, 240));
            }
        }
    }
    // The points.
    for (i, pt) in draft.points.iter().enumerate() {
        let Some(room) = calib::to_room(&show.stage3d, pt) else {
            continue;
        };
        let room = room.map(|v| v as f32);
        if let Some(at) = model_pos(&camera, rect, room)
            && rect.contains(at)
        {
            mark(&painter, at, i, draft.active == Some(i), pt.on);
        }
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
}

/// A click on the model at `at`: the point near it is picked, else a new
/// point is dropped on the screen there (snapped to a corner unless `free`).
#[allow(clippy::too_many_arguments)]
fn click_model(
    show: &Show,
    st: &UiState,
    panes: &mut Panes,
    draft: &mut LineUpDraft,
    p: &Projector,
    camera: &Camera,
    rect: Rect,
    at: Pos2,
    free: bool,
) {
    // A point already there.
    let near = draft
        .points
        .iter()
        .enumerate()
        .filter_map(|(i, pt)| {
            let room = calib::to_room(&show.stage3d, pt)?.map(|v| v as f32);
            let pos = model_pos(camera, rect, room)?;
            Some((i, pos.distance(at)))
        })
        .filter(|(_, d)| *d <= PICK_PT)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((i, _)) = near {
        draft.active = Some(i);
        panes.want_focus = true;
        return;
    }
    let scene = st.stage.scene.clone();
    let ray = camera.ray([at.x - rect.min.x, at.y - rect.min.y], size_of(rect));
    // The screens only (they are small; the venue's ray tests stay off this
    // thread).
    let from = show.stage3d.projectors_from(&scene);
    let Some(hit) = render3d::pick(&scene, &ray, &|i, part| !part.lit && i < from) else {
        return;
    };
    let Some(sid) = show.stage3d.part_surface(&scene, hit.part) else {
        return;
    };
    let Some(s) = show.stage3d.surface(sid) else {
        return;
    };
    let m = s.placement.matrix();
    let snapped = if free {
        None
    } else {
        corners(&s.shape)
            .into_iter()
            .filter_map(|c| {
                let pos = model_pos(camera, rect, geometry::point(&m, c))?;
                Some((c, pos.distance(at)))
            })
            .filter(|(_, d)| *d <= SNAP_PT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c)
    };
    let local = match snapped {
        Some(c) => c,
        None => match calib::to_screen(&show.stage3d, Some(sid), hit.point.map(|v| v as f64)) {
            Some(l) => l,
            None => return,
        },
    };
    let room = geometry::point(&m, local);
    if draft.points.len() >= crate::net::lineup::MAX_MARKS {
        panes.note = Some(format!(
            "At most {} points: switch one off or remove it first.",
            crate::net::lineup::MAX_MARKS
        ));
        return;
    }
    let pixel = first_pixel(show, p, panes, draft, room);
    draft.points.push(LinePoint::on_screen(sid, local, pixel));
    draft.active = Some(draft.points.len() - 1);
    panes.want_focus = true;
}

/// A numbered point drawn at `at` in a pane.
fn mark(painter: &egui::Painter, at: Pos2, i: usize, active: bool, on: bool) {
    let colour = if !on {
        Color32::from_gray(120)
    } else if active {
        Color32::from_rgb(255, 220, 0)
    } else {
        Color32::WHITE
    };
    let ring = egui::Stroke::new(2.0, colour);
    painter.circle_stroke(at, 7.0, egui::Stroke::new(4.0, Color32::from_black_alpha(160)));
    painter.circle_stroke(at, 7.0, ring);
    if active {
        for d in [vec2(1.0, 0.0), vec2(0.0, 1.0)] {
            painter.line_segment([at - d * 12.0, at + d * 12.0], egui::Stroke::new(1.5, colour));
        }
    }
    painter.text(
        at + vec2(9.0, -9.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{}", i + 1),
        egui::FontId::proportional(14.0),
        colour,
    );
}

/// The picture's place in the right pane: its pixels' shape, fitted.
pub(super) fn picture_rect(area: Rect, pixels: (u32, u32)) -> Rect {
    super::preview::fit_picture((pixels.0.max(1), pixels.1.max(1)), area.shrink(4.0))
}

/// The right pane: the projector's picture, the points in it, dragged or
/// moved with the arrow keys.
fn picture_pane(ui: &mut Ui, panes: &mut Panes, draft: &mut LineUpDraft, editable: bool) {
    let area = ui.max_rect();
    let rect = picture_rect(area, draft.pixels);
    let id = egui::Id::new("lineup_picture");
    let response = ui.interact(area, id, Sense::click_and_drag());
    super::stage_edit::note_rect(ui, "Line-up ▸ picture", rect);
    let (w, h) = (draft.pixels.0.max(1) as f32, draft.pixels.1.max(1) as f32);
    let scale = rect.width() / w;
    let to_pos = |px: [f32; 2]| rect.min + vec2(px[0], px[1]) * scale;
    let to_px = |pos: Pos2| {
        let v = (pos - rect.min) / scale;
        [v.x.clamp(0.0, w), v.y.clamp(0.0, h)]
    };
    let nearest = |pos: Pos2, draft: &LineUpDraft| {
        draft
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| (i, to_pos(p.pixel).distance(pos)))
            .filter(|(_, d)| *d <= PICK_PT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    if editable {
        if response.drag_started()
            && let Some(at) = response.interact_pointer_pos()
        {
            let from = ui.input(|i| i.pointer.press_origin()).unwrap_or(at);
            if let Some(i) = nearest(from, draft) {
                draft.active = Some(i);
                panes.drag = Some((i, to_pos(draft.points[i].pixel) - from));
            }
            response.request_focus();
        }
        if response.dragged()
            && let Some((i, off)) = panes.drag
            && let Some(at) = response.interact_pointer_pos()
            && let Some(pt) = draft.points.get_mut(i)
        {
            pt.pixel = to_px(at + off);
        }
        if response.drag_stopped() {
            panes.drag = None;
        }
        if response.clicked() {
            if let Some(at) = response.interact_pointer_pos()
                && let Some(i) = nearest(at, draft)
            {
                draft.active = Some(i);
            }
            response.request_focus();
        }
        if std::mem::take(&mut panes.want_focus) {
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
            let moved = ui.input_mut(|i| arrows(&mut i.events));
            if moved != [0.0, 0.0]
                && let Some(pt) = draft.active.and_then(|a| draft.points.get_mut(a))
            {
                pt.pixel = [
                    (pt.pixel[0] + moved[0]).clamp(0.0, w),
                    (pt.pixel[1] + moved[1]).clamp(0.0, h),
                ];
            }
        }
    }
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, Color32::from_rgb(28, 29, 33));
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(
            if response.has_focus() { 2.0 } else { 1.0 },
            if response.has_focus() {
                Color32::from_rgb(255, 210, 90)
            } else {
                Color32::from_gray(90)
            },
        ),
        egui::StrokeKind::Outside,
    );
    if let Some(o) = &draft.outline {
        let stroke = egui::Stroke::new(1.2, Color32::from_rgb(0, 200, 240));
        for piece in o {
            let pts: Vec<Pos2> = piece.iter().map(|p| to_pos(*p)).collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
    }
    for (i, pt) in draft.points.iter().enumerate() {
        mark(&painter, to_pos(pt.pixel), i, draft.active == Some(i), pt.on);
    }
    painter.text(
        area.left_bottom() + vec2(6.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{} × {} pixels", draft.pixels.0, draft.pixels.1),
        egui::FontId::proportional(11.0),
        Color32::from_gray(150),
    );
}

/// The arrow keys pressed this frame (held keys' repeats too) as a move in
/// pixels, taken out of `events`: one pixel each, Shift ten, Alt a tenth.
pub fn arrows(events: &mut Vec<egui::Event>) -> [f32; 2] {
    let mut d = [0.0f32, 0.0];
    events.retain(|e| {
        let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = e
        else {
            return true;
        };
        let step = if modifiers.alt {
            STEP_ALT
        } else if modifiers.shift {
            STEP_SHIFT
        } else {
            STEP
        };
        let (k, s) = match key {
            egui::Key::ArrowLeft => (0, -step),
            egui::Key::ArrowRight => (0, step),
            egui::Key::ArrowUp => (1, -step),
            egui::Key::ArrowDown => (1, step),
            _ => return true,
        };
        d[k] += s;
        false
    });
    d
}

/// Each point: its number, where it is, how far off the solve leaves it,
/// switched on or off, removed.
fn point_rows(
    ui: &mut Ui,
    show: &Show,
    panes: &mut Panes,
    draft: &mut LineUpDraft,
    editable: bool,
) {
    if draft.points.is_empty() {
        hint(ui, "No points yet: click a corner of a screen on the model, on the left.");
        return;
    }
    let residuals: Vec<Option<f64>> = match panes.solve.as_ref().map(|s| &s.result) {
        Some(Ok(sol)) => sol.residuals.clone(),
        _ => Vec::new(),
    };
    let mut remove = None;
    egui::Grid::new("lineup_points_grid")
        .num_columns(5)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for (i, pt) in draft.points.iter_mut().enumerate() {
                let label = format!("Point {}", i + 1);
                let r = ui.selectable_label(draft.active == Some(i), &label);
                super::stage_edit::note(ui, &format!("Line-up ▸ {label}"), &r);
                if r.clicked() {
                    draft.active = Some(i);
                    panes.want_focus = true;
                }
                let on = pt
                    .surface
                    .and_then(|s| show.stage3d.surface(s))
                    .map_or("the room".to_string(), |s| s.name.clone());
                ui.label(format!(
                    "on {on}, at {:.1}, {:.1} in the picture",
                    pt.pixel[0], pt.pixel[1]
                ));
                match residuals.get(i).copied().flatten() {
                    Some(e) => ui.label(format!("{e:.1} px off")),
                    None => ui.label(""),
                };
                ui.add_enabled_ui(editable, |ui| {
                    let r = ui.checkbox(&mut pt.on, "On");
                    super::stage_edit::note(ui, &format!("Line-up ▸ {label} ▸ On"), &r);
                    let r = ui.small_button("Remove");
                    super::stage_edit::note(ui, &format!("Line-up ▸ {label} ▸ Remove"), &r);
                    if r.clicked() {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        draft.points.remove(i);
        draft.active = match draft.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            a => a,
        };
    }
}

/// The view's picture as a layer filling its own canvas.
fn picture_group(t: &Arc<crate::gpu::LayerTexture>) -> OutputGroup {
    OutputGroup {
        layers: vec![OutputLayer {
            texture: t.clone(),
            opacity: 1.0,
            fit: crate::show::Fit::Stretch,
            transform: Default::default(),
            color: Default::default(),
            fx: Default::default(),
        }],
        geometry: crate::gpu::Geometry {
            canvas: (t.width, t.height),
            warp: crate::gpu::IDENTITY,
            pin: crate::gpu::IDENTITY,
            mesh: None,
            slice: crate::gpu::Geometry::WHOLE,
        },
    }
}

/// On a projector's row (Settings ▸ 3D stage ▸ Projectors): *Line up…*
/// (asking first for one whose display it blanks), the last line-up in
/// words, *Back to before line-up*, and the offer to carry on a draft.
pub(super) fn row_buttons(ui: &mut Ui, show: &mut Show, st: &mut UiState, id: ProjectorId) {
    let Some(p) = show.projector(id).cloned() else {
        return;
    };
    let lining = st.line_up.as_ref().is_some_and(|d| d.projector == id);
    let why = refused(st);
    ui.horizontal_wrapped(|ui| {
        if lining {
            ui.label(RichText::new("Being lined up: the panes are above.").italics());
            return;
        }
        let r = ui
            .add_enabled(why.is_none() && st.line_up.is_none(), egui::Button::new(LINE_UP))
            .on_hover_text(
                "Find where this projector really is from points: click a spot on the model, \
                 move its cross on the wall onto it, four times or more",
            )
            .on_disabled_hover_text(why.unwrap_or("Another projector is being lined up"));
        super::stage_edit::note(ui, &format!("Line up {}", p.name), &r);
        if r.clicked() {
            if p.display.is_some() {
                st.stage.projectors.lineup.asking = Some(id);
            } else {
                start(show, st, id);
            }
        }
        if let Some(lu) = &p.line_up
            && let Some(b) = &lu.before
            && (b.placement != p.placement || b.lens != p.lens)
        {
            let r = ui
                .add_enabled(why.is_none(), egui::Button::new(BACK))
                .on_hover_text(
                    "Put it back where it stood, with the lens it had, before it was first \
                     lined up (the points are kept)",
                )
                .on_disabled_hover_text(why.unwrap_or_default());
            super::stage_edit::note(ui, &format!("{BACK} {}", p.name), &r);
            if r.clicked() {
                let b = b.clone();
                let time = ui.input(|i| i.time);
                back_to_before(show, st, id, &b, time);
            }
        }
    });
    if let Some(w) = why {
        warning(ui, w);
    }
    if st.stage.projectors.lineup.asking == Some(id) && st.line_up.is_none() {
        warning(ui, &blanks_words(&p.name));
        ui.horizontal(|ui| {
            let r = ui.button(BLANK_YES);
            super::stage_edit::note(ui, &format!("{BLANK_YES} {}", p.name), &r);
            if r.clicked() {
                start(show, st, id);
            }
            let r = ui.button(BLANK_NO);
            super::stage_edit::note(ui, &format!("{BLANK_NO} {}", p.name), &r);
            if r.clicked() {
                st.stage.projectors.lineup.asking = None;
            }
        });
    }
    if let Some(lu) = &p.line_up {
        let n = lu.points.iter().filter(|p| p.on).count();
        let words = match &lu.solved {
            Some(s) if lu.is_current() => format!(
                "Lined up from {n} points, to within about {:.1} pixels (worst {:.1}).",
                s.rms_px, s.max_px
            ),
            _ if !lu.points.is_empty() => format!(
                "{n} line-up points kept; not lined up from them as they are: open Line up… to \
                 check them."
            ),
            _ => String::new(),
        };
        if !words.is_empty() {
            hint(ui, &words);
        }
    }
}

/// The offer to carry on a line-up from before (a restart, or one put away),
/// at the top of the 3D stage page; and the draft file looked for once.
pub(super) fn offer(ui: &mut Ui, show: &Show, st: &mut UiState) {
    if st.line_up.is_some() {
        return;
    }
    let panes = &mut st.stage.projectors.lineup;
    if !panes.looked {
        panes.looked = true;
        panes.offer = panes.read(show, &st.show_path);
    }
    if let Some(n) = &st.stage.projectors.lineup.note {
        hint(ui, n);
    }
    let Some(d) = st.stage.projectors.lineup.offer.clone() else {
        return;
    };
    let Some(name) = show.projector(d.projector).map(|p| p.name.clone()) else {
        st.stage.projectors.lineup.offer = None;
        return;
    };
    let why = refused(st);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "Carry on lining up {name}? {} points were placed.",
                d.points.len()
            ))
            .strong(),
        );
        let r = ui
            .add_enabled(why.is_none(), egui::Button::new(CARRY_ON))
            .on_disabled_hover_text(why.unwrap_or_default());
        super::stage_edit::note(ui, "Line-up ▸ Carry on", &r);
        if r.clicked() {
            let mut draft = d.clone();
            // A new session: the crosses of the old one are passed by.
            draft.session = new_session();
            begin(show, st, draft);
        }
        let r = ui.button(FORGET);
        super::stage_edit::note(ui, "Line-up ▸ Forget", &r);
        if r.clicked() {
            st.stage.projectors.lineup.forget_file();
        }
    });
}

#[cfg(test)]
#[path = "stage_lineup_tests.rs"]
mod tests;
