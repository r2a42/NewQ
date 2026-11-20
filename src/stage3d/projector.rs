//! Projectors standing in the room (`Stage3d::projectors`): where each one
//! is, which way it points, its lens and brightness, and the display it is.
//! With them, the throw calculator: the sums a theatre wants before it hires
//! anything (how big the picture is, where to stand, which lens). Pure; the
//! light sums are in `light`.
//!
//! Conventions (tested below):
//! - The lens is at the placement's position and looks along its own -Z; +X
//!   is the picture's right and +Y its top (a glTF camera's convention). With
//!   the sample venues' layout (stage towards -Z, audience towards +Z), a
//!   projector at yaw 0 in the audience points at the stage, square on to a
//!   screen at yaw 0. Yaw, pitch and roll mean what `Placement` says:
//!   positive pitch tips the lens up.
//! - **Throw ratio** is distance over picture width, as on every data sheet.
//!   One metre away the picture is `w1 = 1 / throw` wide and `h1 = w1 /
//!   aspect` high.
//! - **Lens shift** moves the picture's middle off the lens axis, as a share
//!   of the picture's width (x, right) and height (y, up): 0.5 up puts the
//!   picture's bottom edge on the axis. One metre away the picture runs from
//!   `(sx - 0.5) w1` to `(sx + 0.5) w1` across and `(sy - 0.5) h1` to
//!   `(sy + 0.5) h1` up.
//! - **Aspect** is the lens's own (`Lens::aspect`, an anamorphic lens) where
//!   set, else the projector's pixels (its display's, else `resolution`,
//!   else 1920 x 1080). Pixels are square.
//! - Numbers that make no sense are kept within bounds where they are used
//!   (throw 0.1 to 20, lumens 0 to 200,000, shift ±3 pictures), never with a
//!   `clamp` whose bounds come from the file.
//!
//! In the show file every field but `id` has a default and is left out at
//! it, and a field a later NewQ adds is kept (`extra`) and written back.

use super::geometry::{self, Mat4};
use super::{Placement, Surface, SurfaceShape};
use crate::render3d::camera::{Frustum, MAX_SHIFT};
use crate::render3d::{self, Camera, Part, Ray, Scene};
use crate::show::{OutputId, Show};
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct ProjectorId(pub u64);

pub const MIN_THROW: f32 = 0.1;
pub const MAX_THROW: f32 = 20.0;
pub const DEFAULT_THROW: f32 = 1.5;
pub const MAX_LUMENS: f32 = 200_000.0;
pub const DEFAULT_LUMENS: f32 = 5000.0;
pub const DEFAULT_GAMMA: f32 = 2.2;
/// A projector's pixels when nothing says otherwise.
pub const DEFAULT_PIXELS: (u32, u32) = (1920, 1080);
/// How near the lens a picture is drawn from (metres): what the look-through
/// view's near plane is.
pub const LENS_NEAR: f32 = 0.05;

/// How far inside the picture's edge its outline is found (a share of it).
pub const FOOTPRINT_INSET: f32 = 1e-4;

/// Brightness steps offered (lumens).
pub const BRIGHTNESS: [f32; 5] = [3000.0, 5000.0, 10_000.0, 20_000.0, 30_000.0];

/// Projector resolutions offered.
pub const RESOLUTIONS: [(u32, u32); 5] = [
    (1280, 800),
    (1920, 1080),
    (1920, 1200),
    (3840, 2160),
    (4096, 2160),
];

/// A projector's lens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lens {
    /// Distance over picture width.
    #[serde(default = "default_throw", skip_serializing_if = "is_default_throw")]
    pub throw: f32,
    /// The throw ratios a zoom lens goes between (None: a fixed lens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<[f32; 2]>,
    /// The picture moved right and up off the lens axis, as a share of its
    /// width and height.
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub shift: [f32; 2],
    /// The picture's width per height when it isn't the pixels' (an
    /// anamorphic lens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f32>,
    /// How the pixels' values become light (for the edge blend sums later).
    #[serde(default = "default_gamma", skip_serializing_if = "is_default_gamma")]
    pub gamma: f32,
    /// What a later NewQ wrote that this one doesn't know: kept as it was.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_throw() -> f32 {
    DEFAULT_THROW
}

fn is_default_throw(v: &f32) -> bool {
    *v == DEFAULT_THROW
}

fn default_gamma() -> f32 {
    DEFAULT_GAMMA
}

fn is_default_gamma(v: &f32) -> bool {
    *v == DEFAULT_GAMMA
}

fn is_zero2(v: &[f32; 2]) -> bool {
    *v == [0.0; 2]
}

fn default_lumens() -> f32 {
    DEFAULT_LUMENS
}

fn is_default_lumens(v: &f32) -> bool {
    *v == DEFAULT_LUMENS
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

impl Default for Lens {
    fn default() -> Self {
        Lens {
            throw: DEFAULT_THROW,
            zoom: None,
            shift: [0.0; 2],
            aspect: None,
            gamma: DEFAULT_GAMMA,
            extra: serde_json::Map::new(),
        }
    }
}

/// `v` within `lo..=hi`, `or` when it isn't a number.
fn within(v: f32, lo: f32, hi: f32, or: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { or }
}

impl Lens {
    pub fn is_default(&self) -> bool {
        *self == Lens::default()
    }

    /// The throw ratio, kept within `MIN_THROW..=MAX_THROW`.
    pub fn throw(&self) -> f32 {
        within(self.throw, MIN_THROW, MAX_THROW, DEFAULT_THROW)
    }

    /// The zoom's range, smaller first, within the throw's bounds; None for
    /// a fixed lens (or a range that isn't two numbers).
    pub fn zoom(&self) -> Option<(f32, f32)> {
        let [a, b] = self.zoom?;
        if !(a.is_finite() && b.is_finite()) {
            return None;
        }
        let (a, b) = (a.min(b), a.max(b));
        Some((a.clamp(MIN_THROW, MAX_THROW), b.clamp(MIN_THROW, MAX_THROW)))
    }

    /// The shift, kept within ±`MAX_SHIFT`.
    pub fn shift(&self) -> [f32; 2] {
        self.shift.map(|v| within(v, -MAX_SHIFT, MAX_SHIFT, 0.0))
    }

    /// Its own aspect, if it has a sensible one.
    pub fn aspect(&self) -> Option<f32> {
        self.aspect
            .filter(|a| a.is_finite() && (0.2..=10.0).contains(a))
    }

    pub fn gamma(&self) -> f32 {
        within(self.gamma, 1.0, 4.0, DEFAULT_GAMMA)
    }

    /// Whether the lens can give throw ratio `t` (a fixed lens only its own).
    pub fn reaches(&self, t: f32) -> bool {
        match self.zoom() {
            Some((a, b)) => t >= a * 0.999 && t <= b * 1.001,
            None => (t - self.throw()).abs() <= self.throw() * 0.001,
        }
    }

    /// `t` as near as the zoom goes (a fixed lens: its own throw).
    pub fn nearest(&self, t: f32) -> f32 {
        let t = within(t, MIN_THROW, MAX_THROW, self.throw());
        match self.zoom() {
            Some((a, b)) => t.clamp(a, b),
            None => self.throw(),
        }
    }
}

/// The lenses offered, in plain words: no makes or models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensPreset {
    UltraShort,
    Short,
    StandardZoom,
    LongZoom,
    VeryLong,
}

impl LensPreset {
    pub const ALL: [LensPreset; 5] = [
        LensPreset::UltraShort,
        LensPreset::Short,
        LensPreset::StandardZoom,
        LensPreset::LongZoom,
        LensPreset::VeryLong,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LensPreset::UltraShort => "Ultra short throw (0.25)",
            LensPreset::Short => "Short throw (0.5)",
            LensPreset::StandardZoom => "Standard zoom (1.2 to 2.0)",
            LensPreset::LongZoom => "Long zoom (2.0 to 4.0)",
            LensPreset::VeryLong => "Very long zoom (4.0 to 7.0)",
        }
    }

    /// Its throw ratio, or the zoom's range.
    pub fn range(self) -> (f32, f32) {
        match self {
            LensPreset::UltraShort => (0.25, 0.25),
            LensPreset::Short => (0.5, 0.5),
            LensPreset::StandardZoom => (1.2, 2.0),
            LensPreset::LongZoom => (2.0, 4.0),
            LensPreset::VeryLong => (4.0, 7.0),
        }
    }

    /// The lens, set to `throw` where it can zoom there (else its middle).
    pub fn lens(self, throw: Option<f32>) -> Lens {
        let (a, b) = self.range();
        let zoom = (a != b).then_some([a, b]);
        let mut lens = Lens {
            throw: (a + b) / 2.0,
            zoom,
            ..Lens::default()
        };
        if let Some(t) = throw {
            lens.throw = lens.nearest(t);
        }
        lens
    }

    /// The preset a lens is (its throw, or its zoom, matching), if any.
    pub fn of(lens: &Lens) -> Option<LensPreset> {
        LensPreset::ALL.into_iter().find(|p| {
            let (a, b) = p.range();
            match lens.zoom() {
                Some((x, y)) => (x - a).abs() < 1e-3 && (y - b).abs() < 1e-3,
                None => a == b && (lens.throw() - a).abs() < 1e-3,
            }
        })
    }

    /// The preset that gives throw ratio `t`: a zoom whose range holds it,
    /// else a fixed lens within a tenth of it.
    pub fn for_throw(t: f32) -> Option<LensPreset> {
        if !t.is_finite() {
            return None;
        }
        LensPreset::ALL.into_iter().find(|p| {
            let (a, b) = p.range();
            if a == b {
                (t - a).abs() <= a * 0.1
            } else {
                t >= a && t <= b
            }
        })
    }
}

/// A projector in the room.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projector {
    /// From the show's ids (`Show::alloc_id`).
    #[serde(default)]
    pub id: ProjectorId,
    #[serde(default)]
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Where the lens is, and its aim.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    #[serde(default, skip_serializing_if = "Lens::is_default")]
    pub lens: Lens,
    /// Its brightness at full white, as its data sheet says.
    #[serde(default = "default_lumens", skip_serializing_if = "is_default_lumens")]
    pub lumens: f32,
    /// The display this projector is (at most one projector each).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<OutputId>,
    /// Its pixels, used while its display's size isn't known (or it has
    /// no display yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
    /// The "My projectors" entry it was made from (words only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What a later NewQ wrote that this one doesn't know: kept as it was.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Where the edge of a projector's picture falls in the room.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Footprint {
    /// The outline where it lands, in pieces: a piece ends where the edge of
    /// the picture meets nothing.
    pub outline: Vec<Vec<[f32; 3]>>,
    /// Where each corner lands (top-left, top-right, bottom-right,
    /// bottom-left), if it lands on anything.
    pub corners: [Option<[f32; 3]>; 4],
}

/// A picture on a flat screen, in the screen's own frame (metres, its middle
/// at 0, +X right, +Y up).
#[derive(Clone, Debug, PartialEq)]
pub struct OnScreen {
    /// The picture's corners on the screen's plane: top-left, top-right,
    /// bottom-right, bottom-left.
    pub corners: [[f32; 2]; 4],
    /// The share of the screen the picture covers (0..1).
    pub fills: f32,
    /// How far the picture goes past each edge of the screen (metres; 0
    /// where it doesn't): left, right, top, bottom.
    pub spill: [f32; 4],
    /// Its width and height across the middle (metres).
    pub size: (f32, f32),
}

impl Projector {
    pub fn new(id: ProjectorId, name: String) -> Projector {
        Projector {
            id,
            name,
            visible: true,
            placement: Placement::default(),
            lens: Lens::default(),
            lumens: DEFAULT_LUMENS,
            display: None,
            resolution: None,
            model: None,
            extra: serde_json::Map::new(),
        }
    }

    /// The lumens, kept within 0..=`MAX_LUMENS`.
    pub fn lumens(&self) -> f32 {
        within(self.lumens, 0.0, MAX_LUMENS, DEFAULT_LUMENS)
    }

    /// Its pixels: `known` (its display's size, where the caller knows it),
    /// else its own `resolution`, else `DEFAULT_PIXELS`.
    pub fn pixels(&self, known: Option<(u32, u32)>) -> (u32, u32) {
        let ok = |s: (u32, u32)| s.0 > 0 && s.1 > 0 && s.0 <= 16384 && s.1 <= 16384;
        known
            .filter(|s| ok(*s))
            .or(self.resolution.filter(|s| ok(*s)))
            .unwrap_or(DEFAULT_PIXELS)
    }

    /// The picture's width per height for pixels `pixels`.
    pub fn aspect(&self, pixels: (u32, u32)) -> f32 {
        self.lens
            .aspect()
            .unwrap_or(pixels.0.max(1) as f32 / pixels.1.max(1) as f32)
    }

    /// The picture's width and height one metre from the lens.
    pub fn at_one_metre(&self, aspect: f32) -> (f32, f32) {
        let w = 1.0 / self.lens.throw();
        (w, w / sane_aspect(aspect))
    }

    /// The picture's edges one metre ahead of the lens, in the lens's frame.
    pub fn frustum(&self, aspect: f32) -> Frustum {
        let (w, h) = self.at_one_metre(aspect);
        let [sx, sy] = self.lens.shift();
        Frustum {
            left: (sx - 0.5) * w,
            right: (sx + 0.5) * w,
            bottom: (sy - 0.5) * h,
            top: (sy + 0.5) * h,
        }
    }

    /// How big the picture is `distance` metres from the lens (measured along
    /// its axis, on a wall square to it).
    pub fn picture_at(&self, distance: f32, aspect: f32) -> (f32, f32) {
        let (w, h) = self.at_one_metre(aspect);
        let d = if distance.is_finite() {
            distance.max(0.0)
        } else {
            0.0
        };
        (w * d, h * d)
    }

    /// How far from a square wall the lens must be for a picture `width`
    /// metres wide.
    pub fn distance_for(&self, width: f32) -> f32 {
        let w = if width.is_finite() {
            width.max(0.0)
        } else {
            0.0
        };
        w * self.lens.throw()
    }

    /// The lens's frame to the world.
    pub fn matrix(&self) -> Mat4 {
        self.placement.matrix()
    }

    pub fn position(&self) -> [f32; 3] {
        let m = self.matrix();
        [m[3][0], m[3][1], m[3][2]]
    }

    /// Which way the lens points (one long).
    pub fn axis(&self) -> [f32; 3] {
        geometry::normalize(geometry::direction(&self.matrix(), [0.0, 0.0, -1.0]))
    }

    /// The picture's up, in the world (one long).
    pub fn up(&self) -> [f32; 3] {
        geometry::normalize(geometry::direction(&self.matrix(), [0.0, 1.0, 0.0]))
    }

    /// The picture's right, in the world (one long).
    pub fn right(&self) -> [f32; 3] {
        geometry::normalize(geometry::direction(&self.matrix(), [1.0, 0.0, 0.0]))
    }

    /// A world point in the lens's frame (the lens at 0, looking down -Z).
    pub fn to_lens(&self, point: [f32; 3]) -> [f32; 3] {
        let d = geometry::sub(point, self.position());
        [
            geometry::dot(d, self.right()),
            geometry::dot(d, self.up()),
            -geometry::dot(d, self.axis()),
        ]
    }

    /// The ray from the lens through the picture's point `(u, v)` (0, 0 its
    /// top-left, 1, 1 its bottom-right).
    pub fn ray(&self, u: f32, v: f32, aspect: f32) -> Ray {
        let f = self.frustum(aspect);
        let x = f.left + u * (f.right - f.left);
        let y = f.top - v * (f.top - f.bottom);
        let dir = geometry::direction(&self.matrix(), [x, y, -1.0]);
        Ray {
            origin: self.position(),
            dir: geometry::normalize(dir),
        }
    }

    /// Where a world point is in the picture (`(u, v)`, 0..1 inside it) and
    /// how far it is in front of the lens along the axis; None behind it.
    pub fn in_picture(&self, point: [f32; 3], aspect: f32) -> Option<([f32; 2], f32)> {
        let p = self.to_lens(point);
        let depth = -p[2];
        if !(depth.is_finite() && depth > 1e-6) {
            return None;
        }
        let (x, y) = (p[0] / depth, p[1] / depth);
        let f = self.frustum(aspect);
        let u = (x - f.left) / (f.right - f.left);
        let v = (f.top - y) / (f.top - f.bottom);
        Some(([u, v], depth))
    }

    /// Whether a world point is lit by the picture (inside it, in front).
    pub fn reaches(&self, point: [f32; 3], aspect: f32) -> bool {
        self.in_picture(point, aspect)
            .is_some_and(|([u, v], _)| (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v))
    }

    /// The camera that sees what the lens throws: its picture's shape, its
    /// shift (the look-through view).
    pub fn camera(&self, aspect: f32) -> Camera {
        let (_, h) = self.at_one_metre(aspect);
        let eye = self.position();
        let axis = self.axis();
        Camera {
            eye,
            target: [eye[0] + axis[0], eye[1] + axis[1], eye[2] + axis[2]],
            up: self.up(),
            fov_y_deg: (2.0 * (h * 0.5).atan()).to_degrees(),
            near: LENS_NEAR,
            far: 5000.0,
            shift: self.lens.shift(),
            aspect: Some(sane_aspect(aspect)),
        }
    }

    /// Turn the projector (keeping its roll and where it stands) so the
    /// middle of its picture, not just its lens axis, falls on `point`.
    /// False, and nothing changed, when `point` is where the lens is.
    pub fn aim_at(&mut self, point: [f32; 3], aspect: f32) -> bool {
        let from = self.position();
        let to = geometry::sub(point, from);
        let len = geometry::dot(to, to).sqrt();
        if !(len.is_finite() && len > 1e-4) {
            return false;
        }
        let want = geometry::normalize(to);
        let angles = |d: [f32; 3]| {
            (
                (-d[0]).atan2(-d[2]).to_degrees(),
                d[1].clamp(-1.0, 1.0).asin().to_degrees(),
            )
        };
        let (wy, wp) = angles(want);
        // First the axis at the point; then, for a shifted lens, step the
        // picture's middle onto it (each step takes off what is left).
        self.placement.yaw = wy;
        self.placement.pitch = wp;
        for _ in 0..40 {
            let mid = self.ray(0.5, 0.5, aspect).dir;
            let (my, mp) = angles(mid);
            let dy = wrap_degrees(wy - my);
            let dp = wp - mp;
            if dy.abs() < 1e-4 && dp.abs() < 1e-4 {
                break;
            }
            self.placement.yaw = wrap_degrees(self.placement.yaw + dy);
            self.placement.pitch = (self.placement.pitch + dp).clamp(-89.9, 89.9);
        }
        true
    }

    /// Where the middle of the picture lands among `scene`'s parts that
    /// `wanted` says yes to.
    pub fn aimed_at(
        &self,
        scene: &Scene,
        wanted: &dyn Fn(usize, &Part) -> bool,
        aspect: f32,
    ) -> Option<render3d::Hit> {
        render3d::pick(scene, &self.ray(0.5, 0.5, aspect), wanted)
    }

    /// Where the edge of the picture lands among `scene`'s parts that
    /// `wanted` says yes to: `per_edge` rays along each side. Costly on a
    /// big venue (a ray test each): made off the main thread by the UI.
    pub fn footprint(
        &self,
        scene: &Scene,
        wanted: &dyn Fn(usize, &Part) -> bool,
        aspect: f32,
        per_edge: usize,
    ) -> Footprint {
        let n = per_edge.clamp(1, 256);
        // Round the picture: along the top, down the right, back along the
        // bottom and up the left, back to the start.
        let mut around = Vec::with_capacity(4 * n + 1);
        for i in 0..n {
            around.push((i as f32 / n as f32, 0.0));
        }
        for i in 0..n {
            around.push((1.0, i as f32 / n as f32));
        }
        for i in 0..n {
            around.push((1.0 - i as f32 / n as f32, 1.0));
        }
        for i in 0..n {
            around.push((0.0, 1.0 - i as f32 / n as f32));
        }
        around.push((0.0, 0.0));
        let mut out = Footprint::default();
        let mut piece: Vec<[f32; 3]> = Vec::new();
        // A hair inside the edge: a picture fitted to a screen exactly has
        // its edge on the screen's, where a ray may pass either side.
        let inside = |t: f32| FOOTPRINT_INSET + t * (1.0 - 2.0 * FOOTPRINT_INSET);
        for (k, (u, v)) in around.into_iter().enumerate() {
            let ray = self.ray(inside(u), inside(v), aspect);
            let hit = render3d::pick(scene, &ray, wanted).map(|h| h.point);
            if k % n == 0 && k / n < 4 {
                out.corners[k / n] = hit;
            }
            match hit {
                Some(p) => piece.push(p),
                None if !piece.is_empty() => out.outline.push(std::mem::take(&mut piece)),
                None => {}
            }
        }
        if !piece.is_empty() {
            out.outline.push(piece);
        }
        // A loop all the way round is one piece, its two ends joined.
        out.outline.retain(|p| p.len() > 1);
        out
    }

    /// The lines of the light's cone: from the lens to each corner of the
    /// picture where it lands (else `reach` metres out), and, for corners
    /// that land on nothing, the picture's edge out there.
    pub fn cone(&self, footprint: &Footprint, aspect: f32, reach: f32) -> Vec<[[f32; 3]; 2]> {
        let lens = self.position();
        let reach = within(reach, 0.1, 10_000.0, 10.0);
        let corner_uv = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let ends: Vec<([f32; 3], bool)> = corner_uv
            .iter()
            .zip(footprint.corners)
            .map(|(&(u, v), hit)| match hit {
                Some(p) => (p, true),
                None => {
                    let r = self.ray(u, v, aspect);
                    (
                        [
                            r.origin[0] + r.dir[0] * reach,
                            r.origin[1] + r.dir[1] * reach,
                            r.origin[2] + r.dir[2] * reach,
                        ],
                        false,
                    )
                }
            })
            .collect();
        let mut lines: Vec<[[f32; 3]; 2]> = ends.iter().map(|(e, _)| [lens, *e]).collect();
        for i in 0..4 {
            let (a, b) = (ends[i], ends[(i + 1) % 4]);
            if !a.1 && !b.1 {
                lines.push([a.0, b.0]);
            }
        }
        lines
    }

    /// The size on the surface of one pixel at a world `point` with surface
    /// `normal` (None: as on a wall square to the lens), in metres: the side
    /// of the patch one pixel lights (a slanting surface stretches it). None
    /// outside the picture or edge on.
    pub fn pixel_size_at(
        &self,
        point: [f32; 3],
        normal: Option<[f32; 3]>,
        pixels: (u32, u32),
        aspect: f32,
    ) -> Option<f32> {
        let (_, depth) = self.in_picture(point, aspect)?;
        let (w1, _) = self.at_one_metre(aspect);
        let square = depth * w1 / pixels.0.max(1) as f32;
        let stretch = match normal {
            None => 1.0,
            Some(n) => {
                let (ci, ca) = super::light::cosines(self, point, n)?;
                (ca / ci).sqrt()
            }
        };
        let s = square * stretch;
        (s.is_finite() && s > 0.0).then_some(s)
    }

    /// Stand square in front of surface `s` (its front, +Z; for a
    /// rear-projection screen, behind it), at the distance where the picture
    /// fills it as fully as it can without spilling over, with the lens
    /// shift it has: the picture's middle on the screen's. A curved or
    /// faceted screen is taken as the box round it, the lens on its axis.
    /// None for a shape this NewQ doesn't know.
    pub fn fit_to(&self, s: &Surface, aspect: f32) -> Option<Placement> {
        let b = surface_box(&s.shape)?;
        let aspect = sane_aspect(aspect);
        let (bw, bh) = (b.1[0] - b.0[0], b.1[1] - b.0[1]);
        let width = bw.min(bh * aspect).max(0.01);
        let height = width / aspect;
        let [sx, sy] = self.lens.shift();
        let rear = s.rear;
        // In the screen's frame. From behind, the picture's right is the
        // screen's left (seen from the projector's side).
        let side = if rear { -1.0 } else { 1.0 };
        let face_z = if rear { b.0[2] } else { b.1[2] };
        let centre = [(b.0[0] + b.1[0]) / 2.0, (b.0[1] + b.1[1]) / 2.0];
        let d = self.distance_for(width);
        let local = [
            centre[0] - side * sx * width,
            centre[1] - sy * height,
            face_z + side * d,
        ];
        let m = s.placement.matrix();
        let mut p = s.placement;
        p.position = geometry::point(&m, local);
        if rear {
            p.yaw += 180.0;
            p.pitch = -p.pitch;
            p.roll = -p.roll;
        }
        // (With the screen tipped or rolled from behind, sum it properly.)
        let fitted = Projector {
            placement: p,
            ..self.clone()
        };
        let axis_world = geometry::normalize(geometry::direction(
            &m,
            [0.0, 0.0, if rear { 1.0 } else { -1.0 }],
        ));
        let up_world = geometry::normalize(geometry::direction(&m, [0.0, 1.0, 0.0]));
        let p = orient(&fitted, axis_world, up_world);
        Some(p)
    }

    /// "I can only hang it here; which lens?": the throw ratio that fills
    /// surface `s` from where the projector stands (the distance to the
    /// screen's middle over the width the picture should have there).
    pub fn throw_for(&self, s: &Surface, aspect: f32) -> Option<f32> {
        let b = surface_box(&s.shape)?;
        let aspect = sane_aspect(aspect);
        let (bw, bh) = (b.1[0] - b.0[0], b.1[1] - b.0[1]);
        let width = bw.min(bh * aspect).max(0.01);
        let face_z = if s.rear { b.0[2] } else { b.1[2] };
        let mid = geometry::point(
            &s.placement.matrix(),
            [(b.0[0] + b.1[0]) / 2.0, (b.0[1] + b.1[1]) / 2.0, face_z],
        );
        let d = geometry::sub(mid, self.position());
        let distance = geometry::dot(d, d).sqrt();
        let t = distance / width;
        (t.is_finite() && t > 0.0).then_some(t)
    }

    /// The picture on a flat screen's plane, in the screen's frame: where
    /// its corners fall, how much of the screen it fills and how far it
    /// spills over each edge. None for another shape, or when some corner
    /// doesn't reach the plane (the screen is edge on or behind the lens).
    pub fn on_flat(&self, s: &Surface, aspect: f32) -> Option<OnScreen> {
        let SurfaceShape::Flat { width, height } = s.shape else {
            return None;
        };
        let (sw, sh) = (geometry::size(width), geometry::size(height));
        let m = s.placement.matrix();
        let origin = geometry::point(&m, [0.0; 3]);
        let xa = geometry::normalize(geometry::direction(&m, [1.0, 0.0, 0.0]));
        let ya = geometry::normalize(geometry::direction(&m, [0.0, 1.0, 0.0]));
        let n = geometry::normalize(geometry::direction(&m, [0.0, 0.0, 1.0]));
        let on_plane = |u: f32, v: f32| -> Option<[f32; 2]> {
            let r = self.ray(u, v, aspect);
            let denom = geometry::dot(r.dir, n);
            if denom.abs() < 1e-6 {
                return None;
            }
            let t = geometry::dot(geometry::sub(origin, r.origin), n) / denom;
            if !(t.is_finite() && t > 0.0) {
                return None;
            }
            let p = [
                r.origin[0] + r.dir[0] * t - origin[0],
                r.origin[1] + r.dir[1] * t - origin[1],
                r.origin[2] + r.dir[2] * t - origin[2],
            ];
            Some([geometry::dot(p, xa), geometry::dot(p, ya)])
        };
        let corners = [
            on_plane(0.0, 0.0)?,
            on_plane(1.0, 0.0)?,
            on_plane(1.0, 1.0)?,
            on_plane(0.0, 1.0)?,
        ];
        let (hw, hh) = (sw / 2.0, sh / 2.0);
        let inside = clip_to_rect(&corners, -hw, hw, -hh, hh);
        let fills = (polygon_area(&inside) / (sw * sh)).clamp(0.0, 1.0);
        let xs = corners.map(|c| c[0]);
        let ys = corners.map(|c| c[1]);
        let lo = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
        let hi = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
        let spill = [
            (-hw - lo(xs)).max(0.0),
            (hi(xs) - hw).max(0.0),
            (hi(ys) - hh).max(0.0),
            (-hh - lo(ys)).max(0.0),
        ];
        let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let dist =
            |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        let size = (
            dist(mid(corners[0], corners[3]), mid(corners[1], corners[2])),
            dist(mid(corners[0], corners[1]), mid(corners[3], corners[2])),
        );
        Some(OnScreen {
            corners,
            fills,
            spill,
            size,
        })
    }
}

/// Turn `p` so the lens looks along `axis` with its picture's up as near
/// `up` as it can be (keeping where it stands).
fn orient(p: &Projector, axis: [f32; 3], up: [f32; 3]) -> Placement {
    let mut out = p.placement;
    let yaw = (-axis[0]).atan2(-axis[2]).to_degrees();
    let pitch = axis[1].clamp(-1.0, 1.0).asin().to_degrees();
    out.yaw = yaw;
    out.pitch = pitch;
    out.roll = 0.0;
    // The roll that brings the picture's up to `up`: the angle between the
    // unrolled up and `up`, about the axis.
    let unrolled = Projector {
        placement: out,
        ..p.clone()
    };
    let u0 = unrolled.up();
    let r0 = unrolled.right();
    let roll = geometry::dot(up, r0)
        .atan2(geometry::dot(up, u0))
        .to_degrees();
    // Positive roll turns +X towards +Y, so the up turns towards -X.
    out.roll = -roll;
    if out.roll.abs() < 1e-3 {
        out.roll = 0.0;
    }
    out
}

fn wrap_degrees(d: f32) -> f32 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

pub(crate) fn sane_aspect(a: f32) -> f32 {
    if a.is_finite() && (0.1..=20.0).contains(&a) {
        a
    } else {
        16.0 / 9.0
    }
}

/// The box round a shape in its own frame (min, max), None for an unknown
/// one.
pub fn surface_box(shape: &SurfaceShape) -> Option<([f32; 3], [f32; 3])> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for piece in geometry::surface_mesh(shape) {
        for p in &piece.mesh.positions {
            let q = geometry::point(&piece.transform, *p);
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
    }
    (lo.iter().chain(&hi).all(|v| v.is_finite())).then_some((lo, hi))
}

/// The part of convex polygon `poly` inside the rectangle (Sutherland and
/// Hodgman).
fn clip_to_rect(poly: &[[f32; 2]], x0: f32, x1: f32, y0: f32, y1: f32) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = poly.to_vec();
    let edges: [(usize, f32, bool); 4] =
        [(0, x0, true), (0, x1, false), (1, y0, true), (1, y1, false)];
    for (axis, at, keep_above) in edges {
        let inside = |p: &[f32; 2]| {
            if keep_above {
                p[axis] >= at
            } else {
                p[axis] <= at
            }
        };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, b) = (input[i], input[(i + 1) % input.len()]);
            let (ia, ib) = (inside(&a), inside(&b));
            if ia {
                out.push(a);
            }
            if ia != ib {
                let t = (at - a[axis]) / (b[axis] - a[axis]);
                out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

fn polygon_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let twice: f32 = (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    (twice / 2.0).abs()
}

impl Show {
    pub fn projector(&self, id: ProjectorId) -> Option<&Projector> {
        self.stage3d.projectors.iter().find(|p| p.id == id)
    }

    pub fn projector_mut(&mut self, id: ProjectorId) -> Option<&mut Projector> {
        self.stage3d.projectors.iter_mut().find(|p| p.id == id)
    }

    /// Add a projector named "Projector N", standing 10 m in front of the
    /// middle of the room at head height, pointing at the stage.
    pub fn add_projector(&mut self) -> ProjectorId {
        let id = ProjectorId(self.alloc_id());
        let n = (1..)
            .find(|n| {
                let name = format!("Projector {n}");
                !self.stage3d.projectors.iter().any(|p| p.name == name)
            })
            .unwrap_or(1);
        let mut p = Projector::new(id, format!("Projector {n}"));
        p.placement.position = [0.0, 2.0, 10.0];
        self.stage3d.projectors.push(p);
        id
    }

    pub fn remove_projector(&mut self, id: ProjectorId) {
        self.stage3d.projectors.retain(|p| p.id != id);
    }

    /// Why display `output` can't be projector `for_projector`'s (None: it
    /// can): a sub display is an area of a picture, a display that shows the
    /// stage would throw the stage onto itself, and a display is one
    /// projector at most.
    pub fn projector_display_refused(
        &self,
        output: OutputId,
        for_projector: ProjectorId,
    ) -> Option<&'static str> {
        let o = self.output(output)?;
        if o.is_sub_display() {
            Some("A sub display is an area of another display's picture, not a projector.")
        } else if self.shows_stage(output) {
            Some("This display shows the 3D stage: it can't also be a projector in it.")
        } else if self
            .stage3d
            .projectors
            .iter()
            .any(|p| p.id != for_projector && p.display == Some(output))
        {
            Some("Another projector is this display already.")
        } else {
            None
        }
    }

    /// Put right what a hand-edited (or a later NewQ's) file may say of the
    /// projectors: a display that is two projectors (the second lets it go),
    /// or one that can't be a projector.
    pub(super) fn sanitize_projectors(&mut self) {
        for i in 0..self.stage3d.projectors.len() {
            let p = &self.stage3d.projectors[i];
            let Some(o) = p.display else {
                continue;
            };
            let earlier = self.stage3d.projectors[..i]
                .iter()
                .any(|q| q.display == Some(o));
            let bad = self
                .output(o)
                .is_some_and(|c| c.is_sub_display() || self.shows_stage(o));
            if earlier || bad {
                log::warn!(
                    "3D stage: projector {} can't be display {}; it is left without one",
                    p.name,
                    o.0
                );
                self.stage3d.projectors[i].display = None;
            }
        }
    }

    /// After loading: a projector with no id (or one another thing has)
    /// gets a new one.
    pub(crate) fn number_projectors(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.stage3d.projectors.len() {
            let id = self.stage3d.projectors[i].id;
            if id.0 == 0 || !seen.insert(id) {
                let fresh = ProjectorId(self.alloc_id());
                seen.insert(fresh);
                self.stage3d.projectors[i].id = fresh;
            }
        }
    }
}

#[cfg(test)]
#[path = "projector_tests.rs"]
mod tests;
