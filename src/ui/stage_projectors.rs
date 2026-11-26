//! Projectors in the 3D stage: the Projectors tab of Settings ▸ 3D
//! stage (add one aimed at a screen, choose its lens and brightness, stand
//! it and point it, read how big, sharp and bright its picture is, fit it to
//! a screen, ask which lens to hire), and what the 3D views show of them:
//! each one's light as lines (its cone, and its picture's outline where it
//! lands), the look through its lens, and the light under the pointer.
//!
//! The sums are `stage3d::projector` and `stage3d::light`. Anything that
//! tests rays against the venue (an outline is a hundred rays; a venue can
//! be millions of triangles) is done on a worker thread (`Worker`), latest
//! wins, and the last answer is shown meanwhile: the main thread, which
//! feeds the outputs, never waits for it. The row read-outs only test the
//! screens, which are small.
//!
//! The lines go to the views through their `ViewRequest`s, so they are drawn
//! only when the preview governor lets a view draw; a display that shows the
//! stage never has them.
//!
//! "My projectors" (a lens, brightness and pixels kept under a name) are
//! kept per computer in `my-projectors.json` in the log folder, beside
//! `layout.json`, never in the show.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use egui::{Color32, RichText, Ui};
use serde::{Deserialize, Serialize};

use super::UiState;
use super::settings::{hint, untitled, warning};
use crate::preview_pace::PreviewId;
use crate::render3d::{self, Camera, Line, Paint, Part, Ray, Scene};
use crate::show::{DisplayState, OutputId, Show};
use crate::stage3d::light::{self, Facing};
use crate::stage3d::projector::MappedRefused;
use crate::stage3d::projector::{
    self, BRIGHTNESS, Footprint, Lens, LensPreset, MAX_LUMENS, MAX_THROW, MIN_THROW, RESOLUTIONS,
};
use crate::stage3d::{
    LengthUnit, Projector, ProjectorId, Stage3d, Surface, SurfaceId, SurfaceShape,
};

/// What the page says where projectors are added.
pub const CORNER_PIN_WORDS: &str = "For one flat screen, the four corners on the Displays page \
     do the same job with less work. Projectors here are for planning: where to hang one, which \
     lens, how big and how bright the picture is, on any screen or set. Ticked Mapped, a \
     projector shows the canvases of the screens it lights, as they land on them.";

/// Rays along each side of a picture for its outline.
pub const OUTLINE_RAYS: usize = 24;

/// How far a cone is drawn out where its picture lands on nothing (metres).
pub const REACH: f32 = 30.0;

/// The picked projector's lines.
pub const PICKED: [f32; 4] = [1.0, 0.82, 0.25, 0.95];
/// The others'.
pub const OTHERS: [f32; 4] = [0.55, 0.75, 1.0, 0.5];

/// A projector kept under a name on this computer ("My projectors").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub name: String,
    #[serde(default)]
    pub lens: Lens,
    #[serde(default = "default_lumens")]
    pub lumens: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
}

fn default_lumens() -> f32 {
    projector::DEFAULT_LUMENS
}

const LIBRARY_FILE: &str = "my-projectors.json";

/// "My projectors" as saved in `dir` (none if there is no file or it can't
/// be read: a bad file is said in the log, never trusted).
pub fn load_library(dir: &std::path::Path) -> Vec<Saved> {
    let path = dir.join(LIBRARY_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        log::warn!("{} isn't readable ({e}); starting afresh", path.display());
        Vec::new()
    })
}

pub fn save_library(dir: &std::path::Path, list: &[Saved]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{LIBRARY_FILE}.tmp"));
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(list).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&tmp, dir.join(LIBRARY_FILE))
}

/// The light under the pointer, as the worker found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Here {
    /// What was asked (`HereAsk::key`).
    pub key: u64,
    /// What the pointer is on: a screen's name, or "the venue".
    pub on: String,
    /// Each projector lighting it with nothing in the way, by name, and its lux.
    pub lit: Vec<(String, f32)>,
    pub lux: f32,
    pub nits: f32,
    /// One pixel's size there (metres), of the brightest projector.
    pub pixel: Option<f32>,
}

/// What a part of the scene is, for the light sums: a screen's name, gain
/// and facing, or the venue's reflectance.
#[derive(Clone, Debug)]
enum PartKind {
    Venue(f32),
    Screen {
        name: String,
        gain: f32,
        facing: Facing,
    },
}

enum Ask {
    Footprint {
        id: ProjectorId,
        key: u64,
        projector: Box<Projector>,
        aspect: f32,
        scene: Arc<Scene>,
        from: usize,
    },
    Here {
        key: u64,
        ray: Ray,
        scene: Arc<Scene>,
        from: usize,
        kinds: Arc<Vec<PartKind>>,
        projectors: Vec<(Projector, f32, (u32, u32))>,
    },
}

enum Answer {
    Footprint(ProjectorId, u64, Arc<Footprint>),
    Here(u64, Option<Here>),
}

/// The thread the ray tests are done on: latest wins (a footprint per
/// projector, one pointer).
struct Worker {
    tx: Sender<Ask>,
    rx: Receiver<Answer>,
}

impl Worker {
    fn start() -> Option<Worker> {
        let (tx, asks) = std::sync::mpsc::channel::<Ask>();
        let (answers, rx) = std::sync::mpsc::channel::<Answer>();
        std::thread::Builder::new()
            .name("projector-sums".into())
            .spawn(move || {
                let _said = crate::crash::OnPanic(|| {
                    log::warn!("the projectors' sums stopped; they start again when next asked")
                });
                while let Ok(first) = asks.recv() {
                    // Only the latest of each.
                    let mut footprints: HashMap<ProjectorId, Ask> = HashMap::new();
                    let mut here: Option<Ask> = None;
                    let mut take = |a: Ask| match a {
                        Ask::Footprint { id, .. } => {
                            footprints.insert(id, a);
                        }
                        Ask::Here { .. } => here = Some(a),
                    };
                    take(first);
                    while let Ok(more) = asks.try_recv() {
                        take(more);
                    }
                    if let Some(a) = here {
                        let key = match &a {
                            Ask::Here { key, .. } => *key,
                            Ask::Footprint { .. } => 0,
                        };
                        if answers.send(Answer::Here(key, work_here(a))).is_err() {
                            return;
                        }
                    }
                    for (_, a) in footprints {
                        if let Ask::Footprint {
                            id,
                            key,
                            projector,
                            aspect,
                            scene,
                            from,
                        } = a
                        {
                            let fp =
                                projector.footprint(&scene, &|i, _| i < from, aspect, OUTLINE_RAYS);
                            if answers
                                .send(Answer::Footprint(id, key, Arc::new(fp)))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            })
            .ok()?;
        Some(Worker { tx, rx })
    }
}

fn work_here(a: Ask) -> Option<Here> {
    let Ask::Here {
        key,
        ray,
        scene,
        from,
        kinds,
        projectors,
    } = a
    else {
        return None;
    };
    let blocks = |i: usize, _: &Part| i < from;
    let hit = render3d::pick(&scene, &ray, &blocks)?;
    let kind = kinds.get(hit.part).cloned().unwrap_or(PartKind::Venue(0.5));
    // A screen's own front (whichever side the pointer looks at), so a front
    // screen lit from behind is dark from both sides.
    let (on, gain, facing, normal) = match kind {
        PartKind::Venue(r) => ("the venue".to_string(), r, Facing::Both, hit.normal),
        PartKind::Screen { name, gain, facing } => (name, gain, facing, hit.front_normal()),
    };
    let list: Vec<(&Projector, f32)> = projectors.iter().map(|(p, a, _)| (p, *a)).collect();
    let lit = light::lit_by(&list, &scene, &blocks, hit.point, Some(normal), facing);
    let lux: f32 = lit.iter().map(|(_, l)| l).sum();
    let brightest = lit
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(id, _)| projectors.iter().find(|(p, ..)| p.id == *id));
    let pixel = brightest
        .and_then(|(p, aspect, pixels)| p.pixel_size_at(hit.point, Some(normal), *pixels, *aspect));
    let names = lit
        .iter()
        .filter_map(|(id, l)| {
            projectors
                .iter()
                .find(|(p, ..)| p.id == *id)
                .map(|(p, ..)| (p.name.clone(), *l))
        })
        .collect();
    Some(Here {
        key,
        on,
        lit: names,
        lux,
        nits: light::luminance(lux, gain),
        pixel,
    })
}

/// The projectors' part of `StageShared`.
#[derive(Default)]
pub struct ProjectorsUi {
    /// The projector picked (in a view or on the page).
    pub picked: Option<ProjectorId>,
    opened_for: Option<ProjectorId>,
    worker: Option<Worker>,
    /// Each projector's footprint, with the key it was made for.
    footprints: HashMap<ProjectorId, (u64, Arc<Footprint>)>,
    /// Footprints asked for and not yet answered, by key.
    asked: HashMap<ProjectorId, u64>,
    /// What each scene's parts are, for the light sums (by scene key).
    kinds: Option<(u64, Arc<Vec<PartKind>>)>,
    /// The light under the pointer, the view it is under, and what was
    /// last asked.
    pub here: Option<Here>,
    here_for: Option<PreviewId>,
    here_asked: Option<u64>,
    here_waiting: bool,
    /// Each view's lines, with the key they were made for.
    lines: HashMap<Option<ProjectorId>, (u64, Arc<Vec<Line>>)>,
    /// "Which lens from here?" answers, by projector.
    answers: HashMap<ProjectorId, String>,
    /// "My projectors", once read.
    library: Option<Vec<Saved>>,
    /// Where they are kept: the log folder unless set (tests set it).
    pub library_dir: Option<PathBuf>,
    pub library_note: Option<String>,
}

impl ProjectorsUi {
    fn library_dir(&self) -> PathBuf {
        self.library_dir
            .clone()
            .unwrap_or_else(crate::crash::log_dir)
    }

    pub fn library(&mut self) -> &[Saved] {
        if self.library.is_none() {
            self.library = Some(load_library(&self.library_dir()));
        }
        self.library.as_deref().unwrap_or_default()
    }

    fn keep(&mut self, saved: Saved) {
        let mut list = self.library().to_vec();
        list.retain(|s| s.name != saved.name);
        let name = saved.name.clone();
        list.push(saved);
        self.library_note = Some(match save_library(&self.library_dir(), &list) {
            Ok(()) => format!("Kept in My projectors as {name}."),
            Err(e) => format!("Not kept: {e}"),
        });
        self.library = Some(list);
    }

    fn send(&mut self, ask: Ask) {
        if self.worker.is_none() {
            self.worker = Worker::start();
        }
        let Some(w) = &self.worker else {
            return;
        };
        if w.tx.send(ask).is_err() {
            // It stopped (a panic): the next ask starts another.
            self.worker = None;
            self.asked.clear();
            self.here_asked = None;
            self.here_waiting = false;
        }
    }

    /// Take what the worker has finished.
    pub fn take_answers(&mut self) {
        let Some(w) = &self.worker else {
            return;
        };
        loop {
            match w.rx.try_recv() {
                Ok(Answer::Footprint(id, key, fp)) => {
                    if self.asked.get(&id) == Some(&key) {
                        self.asked.remove(&id);
                    }
                    self.footprints.insert(id, (key, fp));
                }
                Ok(Answer::Here(key, h)) => {
                    self.here = h;
                    if self.here_asked == Some(key) {
                        self.here_waiting = false;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.asked.clear();
                    self.here_asked = None;
                    self.here_waiting = false;
                    break;
                }
            }
        }
    }

    /// Projector `p`'s footprint for `key` if it has been worked out; else
    /// it is asked for, and the last one (if any) given meanwhile.
    fn footprint(
        &mut self,
        p: &Projector,
        key: u64,
        aspect: f32,
        scene: &Arc<Scene>,
        from: usize,
    ) -> Option<Arc<Footprint>> {
        let had = self.footprints.get(&p.id).cloned();
        if had.as_ref().is_some_and(|(k, _)| *k == key) {
            return had.map(|(_, f)| f);
        }
        if self.asked.get(&p.id) != Some(&key) {
            self.asked.insert(p.id, key);
            self.send(Ask::Footprint {
                id: p.id,
                key,
                projector: Box::new(p.clone()),
                aspect,
                scene: scene.clone(),
                from,
            });
        }
        // An old outline shown while the new one is made would mislead;
        // the cone alone is drawn until then.
        None
    }

    /// The light under the pointer when it is over view `view`.
    pub fn here_in(&self, view: PreviewId) -> Option<&Here> {
        self.here.as_ref().filter(|_| self.here_for == Some(view))
    }

    /// The pointer has left view `view`: what it said there goes.
    pub fn forget_here(&mut self, view: PreviewId) {
        if self.here_for == Some(view) {
            self.here = None;
            self.here_for = None;
            self.here_asked = None;
        }
    }

    /// Whether the worker has answers to come: the views draw again soon
    /// to show them (nothing else wakes the window when they land).
    pub fn waiting(&self) -> bool {
        self.worker.is_some() && (!self.asked.is_empty() || self.here_waiting)
    }

    /// Whether every footprint asked for has come.
    #[cfg(test)]
    pub fn settled(&self) -> bool {
        self.asked.is_empty()
    }
}

/// A display's size in pixels where it is known: its picture's, else its
/// monitor's here.
pub fn display_pixels(show: &Show, st: &UiState, output: OutputId) -> Option<(u32, u32)> {
    let o = show.output(output)?;
    if let Some(p) = o.picture {
        return Some(p);
    }
    let keys = super::monitor_keys(&st.monitors);
    match crate::show::display_state(o.monitor.as_ref(), o.twins, &keys, None) {
        DisplayState::Connected(i) | DisplayState::OnMainWindowDisplay(i) => {
            st.monitors.get(i).map(|m| m.size)
        }
        _ => None,
    }
}

/// Projector `p`'s pixels and its picture's aspect.
pub fn shape_of(show: &Show, st: &UiState, p: &Projector) -> ((u32, u32), f32) {
    let known = p.display.and_then(|o| display_pixels(show, st, o));
    let pixels = p.pixels(known);
    (pixels, p.aspect(pixels))
}

/// The camera that looks through projector `id`'s lens, if it is there.
pub fn look_through(show: &Show, st: &UiState, id: ProjectorId) -> Option<Camera> {
    let p = show.projector(id)?;
    let (_, aspect) = shape_of(show, st, p);
    Some(p.camera(aspect))
}

fn key_of(scene_key: u64, p: &Projector, aspect: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    scene_key.hash(&mut h);
    // Not its fine-tune or line-up points: they move no footprint.
    format!("{:?}", p.without_tuning()).hash(&mut h);
    aspect.to_bits().hash(&mut h);
    h.finish()
}

/// The lines a view draws over its picture: every shown projector's cone
/// (the picked one's brighter) and its picture's outline where it lands.
/// None of the projector the view looks through (it is the frame).
pub fn view_lines(show: &Show, st: &mut UiState, through: Option<ProjectorId>) -> Arc<Vec<Line>> {
    let stage = &show.stage3d;
    if !stage.projectors.iter().any(|p| p.visible) {
        return Arc::default();
    }
    st.stage.projectors.take_answers();
    let scene = st.stage.scene.clone();
    let from = stage.projectors_from(&scene);
    let picked = st.stage.projectors.picked;
    let shapes: Vec<(ProjectorId, f32, u64)> = stage
        .projectors
        .iter()
        .filter(|p| p.visible && Some(p.id) != through)
        .map(|p| {
            let (_, aspect) = shape_of(show, st, p);
            (p.id, aspect, key_of(st.stage.scene_key, p, aspect))
        })
        .collect();
    let mut all = std::collections::hash_map::DefaultHasher::new();
    let mut footprints = Vec::new();
    for (id, aspect, key) in &shapes {
        let Some(p) = stage.projector(*id) else {
            continue;
        };
        let fp = st
            .stage
            .projectors
            .footprint(p, *key, *aspect, &scene, from);
        key.hash(&mut all);
        fp.is_some().hash(&mut all);
        footprints.push((p, *aspect, fp));
    }
    picked.hash(&mut all);
    let all = all.finish();
    if let Some((k, lines)) = st.stage.projectors.lines.get(&through)
        && *k == all
    {
        return lines.clone();
    }
    let mut lines = Vec::new();
    for (p, aspect, fp) in footprints {
        let colour = if Some(p.id) == picked { PICKED } else { OTHERS };
        let empty = Footprint::default();
        let f = fp.as_deref().unwrap_or(&empty);
        let cone_colour = if Some(p.id) == picked {
            colour
        } else {
            [colour[0], colour[1], colour[2], colour[3] * 0.5]
        };
        for [a, b] in p.cone(f, aspect, REACH) {
            lines.push(Line {
                a,
                b,
                colour: cone_colour,
            });
        }
        for piece in &f.outline {
            for w in piece.windows(2) {
                lines.push(Line {
                    a: w[0],
                    b: w[1],
                    colour,
                });
            }
        }
    }
    let lines = Arc::new(lines);
    st.stage
        .projectors
        .lines
        .insert(through, (all, lines.clone()));
    lines
}

/// What each of `scene`'s parts is for the light sums.
fn part_kinds(stage: &Stage3d, scene: &Scene) -> Vec<PartKind> {
    let venue = scene.parts.iter().take_while(|p| p.lit).count();
    let mut out: Vec<PartKind> = scene.parts[..venue]
        .iter()
        .map(|p| {
            PartKind::Venue(match &p.paint {
                // A display value to light, roughly: its reflectance.
                Paint::Colour(c) => (c[0] + c[1] + c[2]) / 3.0,
                _ => 0.5,
            })
        })
        .collect();
    for s in stage.surfaces.iter().filter(|s| s.visible) {
        let n = crate::stage3d::geometry::surface_mesh(&s.shape).len();
        for _ in 0..n {
            out.push(PartKind::Screen {
                name: s.name.clone(),
                gain: s.gain,
                facing: s.facing(),
            });
        }
    }
    out
}

/// Ask for the light at what `ray` meets (from view `view`'s pointer);
/// the answer comes in `ProjectorsUi::here`, for that view alone
/// (`here_in`).
pub fn ask_here(show: &Show, st: &mut UiState, view: PreviewId, ray: Ray) {
    let stage = &show.stage3d;
    if !stage.projectors.iter().any(|p| p.visible) {
        st.stage.projectors.forget_here(view);
        return;
    }
    if st.stage.projectors.here_for != Some(view) {
        // The pointer has gone to another view: what the last one said
        // isn't about this one.
        st.stage.projectors.here = None;
        st.stage.projectors.here_for = Some(view);
    }
    let scene = st.stage.scene.clone();
    let from = stage.projectors_from(&scene);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    st.stage.scene_key.hash(&mut h);
    for v in ray.origin.iter().chain(&ray.dir) {
        v.to_bits().hash(&mut h);
    }
    format!("{:?}", stage.projectors).hash(&mut h);
    view.hash(&mut h);
    let key = h.finish();
    if st.stage.projectors.here_asked == Some(key) {
        return;
    }
    st.stage.projectors.here_asked = Some(key);
    st.stage.projectors.here_waiting = true;
    let kinds = match &st.stage.projectors.kinds {
        Some((k, kinds)) if *k == st.stage.scene_key => kinds.clone(),
        _ => {
            let kinds = Arc::new(part_kinds(stage, &scene));
            st.stage.projectors.kinds = Some((st.stage.scene_key, kinds.clone()));
            kinds
        }
    };
    let projectors = stage
        .projectors
        .iter()
        .filter(|p| p.visible)
        .map(|p| {
            let (pixels, aspect) = shape_of(show, st, p);
            (p.clone(), aspect, pixels)
        })
        .collect();
    st.stage.projectors.send(Ask::Here {
        key,
        ray,
        scene,
        from,
        kinds,
        projectors,
    });
}

/// The white point a view with its light on takes by itself: the brightest
/// projector's nits where the middle of its picture lands on a screen (on a
/// matt white wall square to it; 10 m out if it lands on none). Only the
/// screens are tested, which are small (never the venue on this thread).
pub fn auto_white(show: &Show, st: &UiState) -> f32 {
    let scene = &st.stage.scene;
    let screens_end = show.stage3d.projectors_from(scene);
    let nits: Vec<f32> = show
        .stage3d
        .projectors
        .iter()
        .filter(|p| p.visible)
        .take(light::MAX_THROWS)
        .map(|p| {
            let (_, aspect) = shape_of(show, st, p);
            let ray = p.ray(0.5, 0.5, aspect);
            let point = render3d::pick(scene, &ray, &|i, part| !part.lit && i < screens_end)
                .map(|h| h.point)
                .unwrap_or_else(|| {
                    render3d::maths::add(ray.origin, render3d::maths::scale(ray.dir, 10.0))
                });
            light::luminance(
                light::illuminance(p, aspect, point, None, Facing::Both),
                1.0,
            )
        })
        .collect();
    light::auto_white(&nits)
}

/// The line under a view for the light under the pointer.
pub fn here_words(h: &Here, unit: LengthUnit) -> String {
    if h.lit.is_empty() {
        return format!("No projector lights this spot on {}", h.on);
    }
    let names: Vec<&str> = h.lit.iter().map(|(n, _)| n.as_str()).collect();
    let mut s = format!(
        "{} · about {} lux · about {} nits",
        names.join(" + "),
        round_words(h.lux),
        round_words(h.nits)
    );
    if let Some(px) = h.pixel {
        s.push_str(&format!(" · 1 pixel ≈ {}", pixel_words(px, unit)));
    }
    s
}

/// A number of lux or nits as people say it: two figures.
pub fn round_words(v: f32) -> String {
    if !v.is_finite() || v <= 0.0 {
        return "0".into();
    }
    let digits = v.log10().floor() as i32;
    let step = 10f32.powi((digits - 1).max(0));
    format!("{}", ((v / step).round() * step) as i64)
}

/// A length for the read-outs, in the stage's units.
pub fn length_words(m: f32, unit: LengthUnit) -> String {
    match unit {
        LengthUnit::Metres if m.abs() < 1.0 => format!("{:.0} cm", m * 100.0),
        LengthUnit::Metres => format!("{m:.1} m"),
        LengthUnit::Centimetres => format!("{:.0} cm", m * 100.0),
        LengthUnit::Millimetres => format!("{:.0} mm", m * 1000.0),
        LengthUnit::Feet | LengthUnit::Inches if m.abs() < 0.3048 => {
            format!("{:.0} in", m / 0.0254)
        }
        LengthUnit::Feet | LengthUnit::Inches => format!("{:.1} ft", m / 0.3048),
    }
}

/// A pixel's size: millimetres, or inches for a stage in feet or inches.
pub fn pixel_words(m: f32, unit: LengthUnit) -> String {
    match unit {
        LengthUnit::Feet | LengthUnit::Inches => format!("{:.2} in", m / 0.0254),
        _ if m < 0.01 => format!("{:.1} mm", m * 1000.0),
        _ => format!("{:.0} mm", m * 1000.0),
    }
}

/// The middle of a screen's front, in the world.
pub fn screen_middle(s: &Surface) -> [f32; 3] {
    let m = s.placement.matrix();
    match projector::surface_box(&s.shape) {
        Some((lo, hi)) => crate::stage3d::geometry::point(
            &m,
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2]],
        ),
        None => s.placement.position,
    }
}

/// The first display that can be a new projector (not taken, not a sub
/// display, not showing the stage).
fn free_display(show: &Show) -> Option<OutputId> {
    show.outputs
        .iter()
        .map(|o| o.id)
        .find(|o| show.projector_display_refused(*o, ProjectorId(0)).is_none())
}

/// Add a projector with a standard zoom, 10,000 lumens, the first free
/// display, stood to fill screen `s` square on. Its id.
pub fn add_aimed_at(show: &mut Show, st: &UiState, s: SurfaceId) -> ProjectorId {
    let display = free_display(show);
    let id = show.add_projector();
    let surface = show.stage3d.surface(s).cloned();
    if let Some(p) = show.projector_mut(id) {
        p.lens = LensPreset::StandardZoom.lens(None);
        p.lumens = 10_000.0;
        p.display = display;
    }
    if let (Some(p), Some(surface)) = (show.projector(id).cloned(), surface) {
        let (_, aspect) = shape_of(show, st, &p);
        if let Some(place) = p.fit_to(&surface, aspect)
            && let Some(p) = show.projector_mut(id)
        {
            p.placement = place;
        }
    }
    id
}

/// Add one of "My projectors", filling the first screen if there is one.
fn add_saved(show: &mut Show, st: &UiState, saved: &Saved) -> ProjectorId {
    let first = show.stage3d.surfaces.first().map(|s| s.id);
    let id = match first {
        Some(s) => add_aimed_at(show, st, s),
        None => show.add_projector(),
    };
    if let Some(p) = show.projector_mut(id) {
        p.lens = saved.lens.clone();
        p.lumens = saved.lumens;
        p.resolution = saved.resolution;
        p.model = Some(saved.name.clone());
    }
    // Stood again for its own lens.
    if let (Some(s), Some(p)) = (
        first.and_then(|s| show.stage3d.surface(s).cloned()),
        show.projector(id).cloned(),
    ) {
        let (_, aspect) = shape_of(show, st, &p);
        if let Some(place) = p.fit_to(&s, aspect)
            && let Some(p) = show.projector_mut(id)
        {
            p.placement = place;
        }
    }
    id
}

/// The Projectors tab of Settings ▸ 3D stage.
pub(super) fn section_ui(ui: &mut Ui, show: &mut Show, st: &mut UiState) {
    untitled(ui, |ui| {
        st.stage.projectors.take_answers();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                add_menu(ui, show, st);
            });
        });
        hint(ui, CORNER_PIN_WORDS);
        if let Some(note) = &st.stage.projectors.library_note {
            hint(ui, note);
        }
        if show.stage3d.projectors.is_empty() {
            hint(
                ui,
                "No projectors yet. Add one aimed at a screen: it is stood where its picture \
                 fills it, and you can then move it, change its lens and see what changes.",
            );
            return;
        }
        let ids: Vec<ProjectorId> = show.stage3d.projectors.iter().map(|p| p.id).collect();
        let picked = st.stage.projectors.picked;
        let newly = picked != st.stage.projectors.opened_for;
        st.stage.projectors.opened_for = picked;
        let mut remove = None;
        for id in ids {
            let is_picked = picked == Some(id);
            let Some(name) = show.projector(id).map(|p| p.name.clone()) else {
                continue;
            };
            let header =
                egui::CollapsingHeader::new(RichText::new(&name).strong().color(if is_picked {
                    Color32::from_rgb(255, 210, 90)
                } else {
                    ui.visuals().text_color()
                }))
                .id_salt(("projector", id.0))
                .open((is_picked && newly).then_some(true));
            let r = header.show(ui, |ui| {
                if row(ui, show, st, id) {
                    remove = Some(id);
                }
            });
            if is_picked && newly {
                r.header_response.scroll_to_me(Some(egui::Align::TOP));
            }
            if r.header_response.clicked() {
                st.stage.projectors.picked = Some(id);
                st.stage.projectors.opened_for = Some(id);
            }
        }
        if let Some(id) = remove {
            show.remove_projector(id);
            if st.stage.projectors.picked == Some(id) {
                st.stage.projectors.picked = None;
            }
            if st.stage.settings_view.through == Some(id) {
                st.stage.settings_view.through = None;
            }
        }
    });
}

fn add_menu(ui: &mut Ui, show: &mut Show, st: &mut UiState) {
    let screens: Vec<(SurfaceId, String)> = show
        .stage3d
        .surfaces
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    ui.menu_button("+ Add a projector", |ui| {
        if screens.is_empty() {
            ui.label(RichText::new("Add a screen first to aim one at it").weak());
        }
        for (s, name) in &screens {
            if ui
                .button(format!("Aimed at {name}"))
                .on_hover_text("Stood square in front of it, where its picture fills it")
                .clicked()
            {
                st.stage.projectors.picked = Some(add_aimed_at(show, st, *s));
                ui.close();
            }
        }
        let saved = st.stage.projectors.library().to_vec();
        if !saved.is_empty() {
            ui.separator();
            ui.label(RichText::new("From My projectors").weak());
            for s in &saved {
                if ui.button(&s.name).clicked() {
                    st.stage.projectors.picked = Some(add_saved(show, st, s));
                    ui.close();
                }
            }
        }
        ui.separator();
        if ui
            .button("Blank")
            .on_hover_text("A projector 10 m out, pointing at the stage")
            .clicked()
        {
            st.stage.projectors.picked = Some(show.add_projector());
            ui.close();
        }
    });
}

/// One projector's settings and read-outs. True when it is to be deleted.
fn row(ui: &mut Ui, show: &mut Show, st: &mut UiState, id: ProjectorId) -> bool {
    let unit = show.stage3d.units;
    let mut delete = false;
    let screens: Vec<Surface> = show.stage3d.surfaces.clone();
    let displays: Vec<(OutputId, String, Option<&'static str>)> = show
        .outputs
        .iter()
        .map(|o| {
            (
                o.id,
                super::display_label(show, o.id),
                show.projector_display_refused(o.id, id),
            )
        })
        .collect();
    let Some(mut p) = show.projector(id).cloned() else {
        return false;
    };
    let before = p.clone();
    let known = p.display.and_then(|o| display_pixels(show, st, o));
    let (pixels, aspect) = shape_of(show, st, &p);
    let through = st.stage.settings_view.through == Some(id);
    let mut look = None;
    let mut keep = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(140.0));
        ui.checkbox(&mut p.visible, "Shown");
        if ui
            .selectable_label(through, "Look through")
            .on_hover_text("See the room as this projector's lens sees it, in the page's 3D view")
            .clicked()
        {
            look = Some(!through);
        }
        if ui
            .button("Keep in My projectors")
            .on_hover_text(
                "Keep its lens, brightness and pixels on this computer, to add again in any show",
            )
            .clicked()
        {
            keep = Some(Saved {
                name: p.model.clone().unwrap_or_else(|| p.name.clone()),
                lens: Lens {
                    extra: Default::default(),
                    ..p.lens.clone()
                },
                lumens: p.lumens,
                resolution: known.or(p.resolution),
            });
        }
        if ui.button("Delete").clicked() {
            delete = true;
        }
    });
    egui::Grid::new(("projector_grid", id.0))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            // Which display it is.
            ui.label("Is display");
            let text = p
                .display
                .and_then(|o| displays.iter().find(|(d, ..)| *d == o))
                .map_or("None yet".to_string(), |(_, n, _)| n.clone());
            egui::ComboBox::from_id_salt(("projector_display", id.0))
                .selected_text(text)
                .width(200.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(p.display.is_none(), "None yet")
                        .clicked()
                    {
                        p.display = None;
                    }
                    for (o, name, refused) in &displays {
                        let r = ui.add_enabled(
                            refused.is_none(),
                            egui::Button::selectable(p.display == Some(*o), name),
                        );
                        let r = match refused {
                            Some(why) => r.on_disabled_hover_text(*why),
                            None => r,
                        };
                        if r.clicked() {
                            p.display = Some(*o);
                        }
                    }
                })
                .response
                .on_hover_text("The display whose picture this projector throws");
            ui.end_row();
            ui.label("Pixels");
            match known {
                Some((w, h)) => {
                    ui.label(format!("{w} × {h}, its display's"));
                }
                None => {
                    let (w, h) = p.pixels(None);
                    egui::ComboBox::from_id_salt(("projector_pixels", id.0))
                        .selected_text(format!("{w} × {h}"))
                        .show_ui(ui, |ui| {
                            for r in RESOLUTIONS {
                                if ui
                                    .selectable_label((w, h) == r, format!("{} × {}", r.0, r.1))
                                    .clicked()
                                {
                                    p.resolution = Some(r);
                                }
                            }
                        });
                }
            }
            ui.end_row();
            lens_rows(ui, id, &mut p.lens);
            ui.label("Brightness");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut p.lumens)
                        .range(0.0..=MAX_LUMENS)
                        .speed(50.0)
                        .max_decimals(0)
                        .suffix(" lumens"),
                )
                .on_hover_text("What its data sheet says (they tend to be generous)");
                ui.menu_button("⏷", |ui| {
                    for b in BRIGHTNESS {
                        if ui.button(format!("{} lumens", b as u32)).clicked() {
                            p.lumens = b;
                            ui.close();
                        }
                    }
                });
            });
            ui.end_row();
            super::stage_edit::placement_rows(ui, &format!("p{}", id.0), &mut p.placement, unit);
            ui.label("Point at");
            egui::ComboBox::from_id_salt(("projector_point", id.0))
                .selected_text("Choose a screen…")
                .show_ui(ui, |ui| {
                    if screens.is_empty() {
                        ui.label(RichText::new("No screens yet").weak());
                    }
                    for s in &screens {
                        if ui.selectable_label(false, &s.name).clicked() {
                            p.aim_at(screen_middle(s), aspect);
                        }
                    }
                })
                .response
                .on_hover_text(
                    "Turn it, where it stands, so its picture's middle is on a screen's",
                );
            ui.end_row();
        });
    // What it gives.
    readouts(ui, show, st, &p, pixels, aspect);
    let canvases_for = mapped_rows(ui, show, &mut p);
    ui.horizontal_wrapped(|ui| {
        ui.menu_button("Fit to a screen ⏷", |ui| {
            if screens.is_empty() {
                ui.label(RichText::new("No screens yet").weak());
            }
            for s in &screens {
                if ui
                    .button(format!("Move it to fill {}", s.name))
                    .on_hover_text(
                        "Stand it square on, at the distance where its picture fills the screen",
                    )
                    .clicked()
                {
                    if let Some(place) = p.fit_to(s, aspect) {
                        p.placement = place;
                    }
                    ui.close();
                }
                if p.lens.zoom().is_some() {
                    let t = p.throw_for(s, aspect);
                    let can = t.is_some_and(|t| p.lens.reaches(t));
                    let r = ui
                        .add_enabled(
                            can,
                            egui::Button::new(format!("Zoom to fill {} from here", s.name)),
                        )
                        .on_disabled_hover_text("This lens doesn't zoom that far");
                    if r.clicked()
                        && let Some(t) = t
                    {
                        p.lens.throw = p.lens.nearest(t);
                        p.aim_at(screen_middle(s), aspect);
                        ui.close();
                    }
                }
            }
        });
        if ui
            .button("Which lens from here?")
            .on_hover_text("The lens that fills the screen it points at, from where it stands")
            .clicked()
        {
            let words = which_lens(show, st, &p, aspect);
            st.stage.projectors.answers.insert(id, words);
        }
    });
    if let Some(words) = st.stage.projectors.answers.get(&id).cloned() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&words).italics());
            if let Some(t) = answer_throw(show, st, &p, aspect)
                && ui
                    .small_button("Use that lens")
                    .on_hover_text("Set this projector's lens to it")
                    .clicked()
            {
                let shift = p.lens.shift;
                p.lens = match LensPreset::for_throw(t) {
                    Some(preset) => preset.lens(Some(t)),
                    None => Lens {
                        throw: t,
                        ..Lens::default()
                    },
                };
                p.lens.shift = shift;
                st.stage.projectors.answers.remove(&id);
            }
        });
    }
    if let Some(on) = look {
        st.stage.settings_view.through = on.then_some(id);
        st.stage.projectors.picked = Some(id);
    }
    if let Some(s) = keep {
        st.stage.projectors.keep(s);
    }
    if p != before
        && let Some(slot) = show.projector_mut(id)
    {
        *slot = p;
    }
    // Screens that showed its display get canvases of their own, and then
    // it is mapped (one edit).
    if !canvases_for.is_empty() {
        for s in canvases_for {
            show.new_canvas_for(s);
        }
        if show.mapped_refused(id).is_none()
            && let Some(p) = show.projector_mut(id)
        {
            p.mapped = true;
        }
    }
    delete
}

/// The line under the *Mapped* tick (old plan 8.1).
pub const MAPPED_WORDS: &str =
    "This display shows the screens as this projector sees them, instead of its own picture.";

/// The small theatre's line (old plan 8.4), for a mapped projector on one
/// flat screen.
pub const ONE_FLAT_SCREEN_WORDS: &str =
    "For one flat screen, the four corners on the Displays page do the same job with less work.";

/// The words of the tick itself.
pub const MAPPED_TICK: &str = "Mapped";

/// Why projector `p` can't be mapped, in words (`Show::mapped_refused`).
pub fn mapped_refusal_words(show: &Show, why: &MappedRefused) -> String {
    match why {
        MappedRefused::NoDisplay => {
            "Choose the display it is first (Is display): mapped, that display shows the \
             stage's screens through this projector."
                .to_string()
        }
        MappedRefused::OnAScreen(screens) => {
            let names = names_of_screens(show, screens);
            format!(
                "{names} {} this display's picture. Mapped, it shows the screens, so it can't \
                 be on one: give {} a canvas of its own (its Shows ▸ {}), or use the button.",
                if screens.len() == 1 { "shows" } else { "show" },
                if screens.len() == 1 { "it" } else { "each" },
                super::stage_edit::NEW_CANVAS,
            )
        }
    }
}

fn names_of_screens(show: &Show, screens: &[SurfaceId]) -> String {
    let names: Vec<&str> = screens
        .iter()
        .filter_map(|s| show.stage3d.surface(*s))
        .map(|s| s.name.as_str())
        .collect();
    and_list(&names)
}

fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A projector's *Mapped* tick and its lines. The screens that are to get a
/// canvas of their own before it is mapped (the caller makes them: the show
/// is read here while `p` is a copy being edited).
fn mapped_rows(ui: &mut Ui, show: &Show, p: &mut Projector) -> Vec<SurfaceId> {
    let mut canvases_for = Vec::new();
    let refused = show.mapped_refused(p.id);
    ui.horizontal_wrapped(|ui| {
        // A mapped one can always be switched off.
        let can = p.mapped || refused.is_none();
        let r = ui.add_enabled(can, egui::Checkbox::new(&mut p.mapped, MAPPED_TICK));
        super::stage_edit::note(ui, &format!("Projectors ▸ {} ▸ {MAPPED_TICK}", p.name), &r);
        let r = r.on_hover_text(MAPPED_WORDS);
        if let Some(why) = &refused {
            r.on_disabled_hover_text(mapped_refusal_words(show, why));
        }
    });
    hint(ui, MAPPED_WORDS);
    if !p.mapped {
        if let Some(why) = &refused {
            hint(ui, &mapped_refusal_words(show, why));
            if let MappedRefused::OnAScreen(screens) = why {
                let words = format!(
                    "Give {} a new canvas, and map it",
                    names_of_screens(show, screens)
                );
                let r = ui.button(&words).on_hover_text(
                    "Each of those screens shows a new canvas instead (cues sent to this \
                     display stay on it, and aren't seen while it is mapped)",
                );
                super::stage_edit::note(
                    ui,
                    &format!("Projectors ▸ {} ▸ Give a canvas", p.name),
                    &r,
                );
                if r.clicked() {
                    canvases_for = screens.clone();
                }
            }
        }
        return canvases_for;
    }
    let feeds = show.mapped_feeds(p.id);
    let name = |o: &OutputId| show.output(*o).map_or("", |o| o.name.as_str());
    if feeds.screens.is_empty() {
        warning(
            ui,
            "Its beam reaches no projection screen that shows a canvas: its display is black. \
             Point it at a screen, or give the screen something to show.",
        );
    } else {
        let used: Vec<&str> = feeds.used.iter().map(name).collect();
        hint(
            ui,
            &format!(
                "Maps {} onto {}.",
                and_list(&used),
                names_of_screens(show, &feeds.screens)
            ),
        );
    }
    if !feeds.left_out.is_empty() {
        let out: Vec<&str> = feeds.left_out.iter().map(name).collect();
        warning(
            ui,
            &format!(
                "A mapped projector shows at most {} canvases; this one sees {}. Where it \
                 meets {} it is black: show fewer canvases on the screens it reaches.",
                crate::mapped::MAX_FEEDS,
                feeds.used.len() + feeds.left_out.len(),
                and_list(&out),
            ),
        );
    }
    let one_flat = feeds.screens.len() == 1
        && show
            .stage3d
            .surface(feeds.screens[0])
            .is_some_and(|s| matches!(s.shape, SurfaceShape::Flat { .. }));
    if one_flat {
        hint(ui, ONE_FLAT_SCREEN_WORDS);
    }
    canvases_for
}

fn lens_rows(ui: &mut Ui, id: ProjectorId, lens: &mut Lens) {
    ui.label("Lens");
    ui.horizontal(|ui| {
        let current = LensPreset::of(lens);
        egui::ComboBox::from_id_salt(("projector_lens", id.0))
            .selected_text(current.map_or("Typed in", LensPreset::label))
            .width(200.0)
            .show_ui(ui, |ui| {
                for preset in LensPreset::ALL {
                    if ui
                        .selectable_label(current == Some(preset), preset.label())
                        .clicked()
                    {
                        let shift = lens.shift;
                        *lens = preset.lens(Some(lens.throw()));
                        lens.shift = shift;
                    }
                }
            })
            .response
            .on_hover_text(
                "The throw ratio: how far away the projector stands for each metre of picture \
                 width. A zoom lens has a range.",
            );
    });
    ui.end_row();
    ui.label("Throw ratio");
    ui.horizontal(|ui| {
        match lens.zoom() {
            Some((a, b)) => {
                let mut t = lens.throw();
                if ui
                    .add(
                        egui::Slider::new(&mut t, a..=b)
                            .max_decimals(2)
                            .clamping(egui::SliderClamping::Always),
                    )
                    .on_hover_text("Where the zoom is set")
                    .changed()
                {
                    lens.throw = t;
                }
            }
            None => {
                ui.add(
                    egui::DragValue::new(&mut lens.throw)
                        .range(MIN_THROW..=MAX_THROW)
                        .speed(0.01)
                        .max_decimals(2),
                );
            }
        }
        ui.label(
            RichText::new(format!(
                "{:.2} m away for each metre of picture",
                lens.throw()
            ))
            .weak(),
        );
        let mut zooms = lens.zoom.is_some();
        if ui
            .checkbox(&mut zooms, "Zooms")
            .on_hover_text("A zoom lens: give the range of throw ratios it goes between")
            .changed()
        {
            let t = lens.throw();
            lens.zoom = zooms.then_some([(t * 0.8).max(MIN_THROW), (t * 1.25).min(MAX_THROW)]);
        }
        if let Some(z) = &mut lens.zoom {
            ui.label(RichText::new("from").weak());
            ui.add(
                egui::DragValue::new(&mut z[0])
                    .range(MIN_THROW..=MAX_THROW)
                    .speed(0.01)
                    .max_decimals(2),
            );
            ui.label(RichText::new("to").weak());
            ui.add(
                egui::DragValue::new(&mut z[1])
                    .range(MIN_THROW..=MAX_THROW)
                    .speed(0.01)
                    .max_decimals(2),
            );
        }
    });
    ui.end_row();
    ui.label("Lens shift");
    ui.horizontal(|ui| {
        let help = "How far the lens moves the picture without tilting the projector, as a \
                    share of the picture: 50% up puts the picture's bottom edge level with the \
                    lens. Data sheets give it in different ways: check yours.";
        let mut up = lens.shift[1] * 100.0;
        let mut right = lens.shift[0] * 100.0;
        ui.label(RichText::new("up").weak());
        if ui
            .add(
                egui::DragValue::new(&mut up)
                    .range(-300.0..=300.0)
                    .speed(0.5)
                    .max_decimals(1)
                    .suffix("%"),
            )
            .on_hover_text(help)
            .changed()
        {
            lens.shift[1] = up / 100.0;
        }
        ui.label(RichText::new("right").weak());
        if ui
            .add(
                egui::DragValue::new(&mut right)
                    .range(-300.0..=300.0)
                    .speed(0.5)
                    .max_decimals(1)
                    .suffix("%"),
            )
            .on_hover_text(help)
            .changed()
        {
            lens.shift[0] = right / 100.0;
        }
    });
    ui.end_row();
}

/// The screen projector `p`'s picture's middle lands on (only the screens
/// are tested: cheap), and the hit.
fn aimed_screen<'a>(
    show: &'a Show,
    st: &UiState,
    p: &Projector,
    aspect: f32,
) -> Option<(&'a Surface, render3d::Hit)> {
    let scene = &st.stage.scene;
    let from = show.stage3d.projectors_from(scene);
    let hit = p.aimed_at(scene, &|i, part| !part.lit && i < from, aspect)?;
    let sid = show.stage3d.part_surface(scene, hit.part)?;
    Some((show.stage3d.surface(sid)?, hit))
}

/// The read-outs under a projector's settings.
fn readouts(
    ui: &mut Ui,
    show: &Show,
    st: &UiState,
    p: &Projector,
    pixels: (u32, u32),
    aspect: f32,
) {
    for line in readout_words(show, st, p, pixels, aspect) {
        match line {
            Said::Fact(s) => {
                ui.label(s);
            }
            Said::Warning(s) => warning(ui, &s),
        }
    }
}

/// A line of the read-outs.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Fact(String),
    Warning(String),
}

/// What a projector gives, in words: the picture on the screen it points
/// at, its brightness and pixel size there, and what is wrong.
pub fn readout_words(
    show: &Show,
    st: &UiState,
    p: &Projector,
    pixels: (u32, u32),
    aspect: f32,
) -> Vec<Said> {
    let unit = show.stage3d.units;
    let mut out = Vec::new();
    if let (Some((a, b)), t) = (p.lens.zoom(), p.lens.throw())
        && !p.lens.reaches(t)
    {
        out.push(Said::Warning(format!(
            "This lens can't zoom that far: it goes from {a:.2} to {b:.2}."
        )));
    }
    let Some((s, hit)) = aimed_screen(show, st, p, aspect) else {
        out.push(Said::Warning(
            "The middle of its picture lands on no screen: point it at one, or fit it to one."
                .into(),
        ));
        return out;
    };
    match p.on_flat(s, aspect) {
        Some(on) => {
            out.push(Said::Fact(format!(
                "Picture on {}: {} by {}, fills {:.0}% of it.",
                s.name,
                length_words(on.size.0, unit),
                length_words(on.size.1, unit),
                (on.fills * 100.0).floor()
            )));
            let edges = ["left", "right", "top", "bottom"];
            let over: Vec<String> = on
                .spill
                .iter()
                .zip(edges)
                .filter(|(v, _)| **v > 0.01)
                .map(|(v, e)| format!("{} over the {e}", length_words(*v, unit)))
                .collect();
            if !over.is_empty() {
                out.push(Said::Warning(format!(
                    "The picture is bigger than the screen: it spills {}.",
                    over.join(", ")
                )));
            }
        }
        None => {
            let depth = p.in_picture(hit.point, aspect).map_or(0.0, |(_, d)| d);
            let (w, h) = p.picture_at(depth, aspect);
            out.push(Said::Fact(format!(
                "Picture where it meets {}: about {} by {}.",
                s.name,
                length_words(w, unit),
                length_words(h, unit)
            )));
        }
    }
    // The screen's own front: the ray from the lens meets a front screen
    // from behind when the projector is behind it.
    let front = hit.front_normal();
    let lux = light::illuminance(p, aspect, hit.point, Some(front), s.facing());
    if lux > 0.0 {
        out.push(Said::Fact(format!(
            "About {} lux there, about {} nits to the audience (if nothing is in the way). {} from the lens.",
            round_words(lux),
            round_words(light::luminance(lux, s.gain)),
            length_words(hit.distance, unit)
        )));
    } else {
        out.push(Said::Warning(format!(
            "It lights {} from behind: a front screen shows nothing that way.",
            s.name
        )));
    }
    if let Some(px) = p.pixel_size_at(hit.point, Some(front), pixels, aspect) {
        out.push(Said::Fact(format!(
            "One pixel is about {} there.",
            pixel_words(px, unit)
        )));
    }
    out
}

/// The screen "Which lens from here?" is about: the one the picture's
/// middle is on, else the nearest to where it points.
fn lens_screen<'a>(
    show: &'a Show,
    st: &UiState,
    p: &Projector,
    aspect: f32,
) -> Option<&'a Surface> {
    if let Some((s, _)) = aimed_screen(show, st, p, aspect) {
        return Some(s);
    }
    let axis = p.axis();
    let from = p.position();
    show.stage3d.surfaces.iter().max_by(|a, b| {
        let along = |s: &Surface| {
            let d = crate::stage3d::geometry::normalize(crate::stage3d::geometry::sub(
                screen_middle(s),
                from,
            ));
            crate::stage3d::geometry::dot(d, axis)
        };
        along(a).total_cmp(&along(b))
    })
}

fn answer_throw(show: &Show, st: &UiState, p: &Projector, aspect: f32) -> Option<f32> {
    let s = lens_screen(show, st, p, aspect)?;
    p.throw_for(s, aspect)
        .filter(|t| (MIN_THROW..=MAX_THROW).contains(t))
}

/// The answer to "Which lens from here?", in words.
pub fn which_lens(show: &Show, st: &UiState, p: &Projector, aspect: f32) -> String {
    let Some(s) = lens_screen(show, st, p, aspect) else {
        return "Add a screen first: the answer depends on how big the picture must be.".into();
    };
    let Some(t) = p.throw_for(s, aspect) else {
        return format!("{} can't be filled from here.", s.name);
    };
    if t > MAX_THROW {
        return format!(
            "From here {} needs a throw ratio of about {t:.1}: further than any usual lens. \
             Move it closer.",
            s.name
        );
    }
    let usual = match LensPreset::for_throw(t) {
        Some(preset) => format!("A {} does it.", preset.label().to_lowercase()),
        None => format!("No usual lens is quite that: look for one of about {t:.2}."),
    };
    let this = if p.lens.reaches(t) {
        " This projector's lens can."
    } else {
        ""
    };
    format!(
        "From here {} needs a throw ratio of about {t:.2} ({t:.2} m away for each metre of \
         picture). {usual}{this}",
        s.name
    )
}

#[cfg(test)]
#[path = "stage_projectors_tests.rs"]
mod tests;
