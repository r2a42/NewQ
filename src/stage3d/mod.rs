//! The 3D stage: a venue model and the screens placed in it, kept in the show
//! (`Show::stage3d`), and what the 3D view draws of them (`scene`).
//!
//! Lengths are in metres, angles in degrees. The world is right-handed with Y
//! up; the audience is usually towards +Z of a screen (its front). A
//! `Placement` is a position and a turn: yaw about Y (positive turns +Z
//! towards +X), then pitch about the turned X (positive tips the top, +Y,
//! towards the front, +Z), then roll about the front (positive turns +X
//! towards +Y). As a matrix: `T * Ry(yaw) * Rx(pitch) * Rz(roll)`.
//!
//! Nothing here is written to a show file while the stage is empty, nor is
//! any field at its default: shows from before 0.25 load and save unchanged.

// The Preview pane's 3D mode and the editing UI use these (integration).
#![allow(dead_code)]

pub mod geometry;
pub mod glb;
pub mod lensmap;
pub mod light;
pub mod lineup_overlay;
pub mod load;
pub mod projector;
pub mod samples;
pub mod scene;

use crate::show::{OutputId, Show};
use geometry::Mat4;
use load::LoadedVenue;
pub use projector::{Projector, ProjectorId};
use scene::{Fnv, Mesh, Paint, Part, Scene};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// What a surface with no picture is painted: a dim grey, so it can still be
/// seen and placed.
pub const UNFED_GREY: [f32; 4] = [0.18, 0.18, 0.18, 1.0];

/// The whole of a picture, as a region.
pub const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// What a projector's body is painted (lit, like the venue).
pub const PROJECTOR_GREY: [f32; 4] = [0.62, 0.64, 0.7, 1.0];

/// Why a mapped projector's display can't show the stage (`stage_source_refused`).
pub const MAPPED_SHOWS_NO_STAGE: &str = "This display is a mapped projector's: it shows the stage's \
     screens through that projector, so it can't show the stage from a viewpoint as well.";

/// A projector's body in its own frame: a box wholly behind the lens (which
/// is at the origin, looking down -Z), so looking through the lens never
/// sees its inside. 44 cm wide, 18 cm high, 45 cm deep.
pub fn projector_body() -> Mesh {
    let (x, y, z0, z1) = (0.22f32, 0.09f32, 0.03f32, 0.48f32);
    let c = |i: usize| {
        [
            if i & 1 == 0 { -x } else { x },
            if i & 2 == 0 { -y } else { y },
            if i & 4 == 0 { z0 } else { z1 },
        ]
    };
    // Each face its own corners, so it is shaded flat; CCW from outside.
    let faces: [([usize; 4], [f32; 3]); 6] = [
        ([0, 2, 3, 1], [0.0, 0.0, -1.0]),
        ([4, 5, 7, 6], [0.0, 0.0, 1.0]),
        ([0, 4, 6, 2], [-1.0, 0.0, 0.0]),
        ([1, 3, 7, 5], [1.0, 0.0, 0.0]),
        ([2, 6, 7, 3], [0.0, 1.0, 0.0]),
        ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
    ];
    let mut m = Mesh::default();
    for (corners, n) in faces {
        let base = m.positions.len() as u32;
        for k in corners {
            m.positions.push(c(k));
            m.normals.push(n);
            m.uvs.push([0.0, 0.0]);
        }
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    m
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stage3d {
    /// The model of the room or the stage the screens are in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<Venue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
    /// Saved camera positions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewpoints: Vec<Viewpoint>,
    /// Projectors standing in the room (`projector`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projectors: Vec<Projector>,
    /// How lengths are shown to the user. Kept in metres whatever this says.
    #[serde(
        default,
        skip_serializing_if = "LengthUnit::is_default",
        deserialize_with = "lenient"
    )]
    pub units: LengthUnit,
    /// What a later NewQ wrote that this one doesn't know: kept as it was.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A value a later NewQ may write in a way this one doesn't know (a new
/// unit, a new axis, a new kind of stage source): read as its default, with
/// a line in the log, rather than refusing the whole show.
pub fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(T::deserialize(&value).unwrap_or_else(|e| {
        log::warn!("3D stage: {value} isn't known to this NewQ ({e}); its default is used");
        T::default()
    }))
}

/// `lenient` for an optional value: one this NewQ doesn't know is None.
pub fn lenient_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(Option::<T>::deserialize(&value).unwrap_or_else(|e| {
        log::warn!("3D stage: {value} isn't known to this NewQ ({e}); it is left out");
        None
    }))
}

/// A length unit: what a model file's numbers are in, or how the stage's
/// lengths are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    #[default]
    Metres,
    Centimetres,
    Millimetres,
    Inches,
    Feet,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 5] = [
        LengthUnit::Metres,
        LengthUnit::Centimetres,
        LengthUnit::Millimetres,
        LengthUnit::Inches,
        LengthUnit::Feet,
    ];

    pub fn is_default(&self) -> bool {
        *self == LengthUnit::Metres
    }

    /// Metres in one of it.
    pub fn metres(self) -> f32 {
        match self {
            LengthUnit::Metres => 1.0,
            LengthUnit::Centimetres => 0.01,
            LengthUnit::Millimetres => 0.001,
            LengthUnit::Inches => 0.0254,
            LengthUnit::Feet => 0.3048,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Metres => "Metres",
            LengthUnit::Centimetres => "Centimetres",
            LengthUnit::Millimetres => "Millimetres",
            LengthUnit::Inches => "Inches",
            LengthUnit::Feet => "Feet",
        }
    }
}

/// Which way is up in a model file. glTF is Y up by its definition; OBJ
/// files from programs that work Z up (3ds Max, SketchUp, some CAD) need Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

impl UpAxis {
    pub fn is_default(&self) -> bool {
        *self == UpAxis::Y
    }

    /// Turns the file's up to +Y: for Z up, -90 degrees about X (+Z to +Y,
    /// +Y to -Z: a model's front, towards -Y in a Z-up program, comes to +Z).
    /// A turn, never a mirror.
    pub fn to_y_up(self) -> Mat4 {
        match self {
            UpAxis::Y => scene::IDENTITY,
            UpAxis::Z => geometry::rotation_x(-90.0),
        }
    }
}

/// Where a thing is and which way it is turned (see the module's words).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Metres.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub position: [f32; 3],
    /// Degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub yaw: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub roll: f32,
}

impl Placement {
    pub fn is_default(&self) -> bool {
        *self == Placement::default()
    }

    /// Model to world (or to the surface, for a facet), column-major.
    /// Non-finite numbers count as 0.
    pub fn matrix(&self) -> Mat4 {
        let f = |v: f32| if v.is_finite() { v } else { 0.0 };
        let t = geometry::translation(self.position.map(f));
        let r = geometry::mul(
            &geometry::rotation_y(f(self.yaw)),
            &geometry::mul(
                &geometry::rotation_x(f(self.pitch)),
                &geometry::rotation_z(f(self.roll)),
            ),
        );
        geometry::mul(&t, &r)
    }
}

fn is_origin(p: &[f32; 3]) -> bool {
    *p == [0.0; 3]
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn one() -> f32 {
    1.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_whole(r: &[f32; 4]) -> bool {
    *r == WHOLE
}

/// The venue: a model file of the room, stage or arena, or one of NewQ's
/// own sample venues (`samples`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Venue {
    /// The model file (.glb, .gltf or .obj). Kept relative to the show file
    /// when saved, as cues' files are. Empty (and unwritten) for a sample.
    #[serde(default, skip_serializing_if = "is_no_path")]
    pub path: PathBuf,
    /// One of NewQ's sample venues, made in code; `path` is unused when set.
    /// Samples are in metres, Y up, whatever `units` and `up` say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<samples::SampleVenue>,
    /// Where the model is put, after its units and up axis.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// What the file's numbers are in (OBJ has no units; glTF is metres).
    #[serde(
        default,
        skip_serializing_if = "LengthUnit::is_default",
        deserialize_with = "lenient"
    )]
    pub units: LengthUnit,
    #[serde(
        default,
        skip_serializing_if = "UpAxis::is_default",
        deserialize_with = "lenient"
    )]
    pub up: UpAxis,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Its colours and pictures multiplied by this (0.05 to 4), to dim a
    /// bright model so the screens stand out. Applied when the model is
    /// loaded (`load_venue`): a change loads it again (`needs_reload`).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub brightness: f32,
}

fn is_no_path(p: &std::path::Path) -> bool {
    p.as_os_str().is_empty()
}

impl Venue {
    pub fn new(path: PathBuf) -> Self {
        Venue {
            path,
            sample: None,
            placement: Placement::default(),
            units: LengthUnit::Metres,
            up: UpAxis::Y,
            visible: true,
            brightness: 1.0,
        }
    }

    /// One of NewQ's sample venues (`samples::sample_venue` makes it).
    pub fn sample(kind: samples::SampleVenue) -> Self {
        Venue {
            sample: Some(kind),
            ..Venue::new(PathBuf::new())
        }
    }

    /// `brightness`, kept within 0.05..=4.
    pub fn brightness(&self) -> f32 {
        if self.brightness.is_finite() {
            self.brightness.clamp(0.05, 4.0)
        } else {
            1.0
        }
    }

    /// Whether a model loaded for `loaded_for` must be loaded again for this
    /// venue (units, up axis and placement don't need it).
    pub fn needs_reload(&self, loaded_for: &Venue) -> bool {
        self.path != loaded_for.path
            || self.sample != loaded_for.sample
            || self.brightness() != loaded_for.brightness()
    }

    /// The model's file coordinates to the world. A sample is in metres and
    /// Y up already: only its placement counts.
    pub fn matrix(&self) -> Mat4 {
        if self.sample.is_some() {
            return self.placement.matrix();
        }
        geometry::mul(
            &self.placement.matrix(),
            &geometry::mul(&self.up.to_y_up(), &geometry::scale(self.units.metres())),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct SurfaceId(pub u64);

/// A screen in the venue: an LED wall, a projection screen, a jumbotron.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// From the show's ids (`Show::alloc_id`), so stable and unique.
    pub id: SurfaceId,
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    pub shape: SurfaceShape,
    #[serde(default, skip_serializing_if = "Feed::is_none")]
    pub feed: Feed,
    /// How much brighter than a matt white surface it looks (the light sums,
    /// `light::luminance`): 1 for matt white.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub gain: f32,
    /// A rear-projection screen: lit from behind, seen from the front.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rear: bool,
    /// Projection or LED, as chosen; None (left out): worked out, once, from
    /// where things stand (`Show::screen_kind`). Mapping plan, section 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SurfaceKind>,
    /// What a later NewQ wrote that this one doesn't know: kept as it was.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// What a screen is, as chosen on it (`Surface::kind`). Its side (front or
/// rear projection) is `Surface::rear`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SurfaceKind {
    /// Projectors light it.
    Projection,
    /// It makes its own light (an LED wall, a monitor): no projector maps
    /// onto it, and the light leaves its picture as it is.
    Led,
    /// A kind a later NewQ wrote that this one doesn't know: kept as it was
    /// written (and written back), worked out here as if none were chosen.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

/// What a screen is, worked out (`Show::screen_kind`): what the light and a
/// mapped projector go by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    /// Projection, lit from the front.
    Front,
    /// Projection, lit from behind.
    Rear,
    /// An LED wall or a monitor: it makes its own light.
    Led,
}

impl ScreenKind {
    /// Its words ("projection, front").
    pub fn words(self) -> &'static str {
        match self {
            ScreenKind::Front => "projection, front",
            ScreenKind::Rear => "projection, rear",
            ScreenKind::Led => "LED or monitor",
        }
    }

    pub fn is_projection(self) -> bool {
        self != ScreenKind::Led
    }
}

/// A screen's kinds as its menu offers them: None is "Works it out".
pub const KIND_CHOICES: [Option<ScreenKind>; 4] = [
    None,
    Some(ScreenKind::Front),
    Some(ScreenKind::Rear),
    Some(ScreenKind::Led),
];

impl Surface {
    /// What was chosen for it, if anything (a kind this NewQ doesn't know:
    /// nothing, so it is worked out).
    pub fn chosen_kind(&self) -> Option<ScreenKind> {
        match self.kind {
            Some(SurfaceKind::Projection) if self.rear => Some(ScreenKind::Rear),
            Some(SurfaceKind::Projection) => Some(ScreenKind::Front),
            Some(SurfaceKind::Led) => Some(ScreenKind::Led),
            Some(SurfaceKind::Unknown(_)) | None => None,
        }
    }

    /// Choose its kind (None: work it out, keeping the side it had). An LED
    /// wall is never rear.
    pub fn choose_kind(&mut self, kind: Option<ScreenKind>) {
        match kind {
            None => self.kind = None,
            Some(ScreenKind::Front) => {
                self.kind = Some(SurfaceKind::Projection);
                self.rear = false;
            }
            Some(ScreenKind::Rear) => {
                self.kind = Some(SurfaceKind::Projection);
                self.rear = true;
            }
            Some(ScreenKind::Led) => {
                self.kind = Some(SurfaceKind::Led);
                self.rear = false;
            }
        }
    }

    /// The boxes its pieces fill in the room, each as its eight corners:
    /// what a projector's beam is tested against (as the light's
    /// `lit::reached` does, but where things stand, not per frame).
    pub fn boxes(&self) -> Vec<[[f32; 3]; 8]> {
        let place = self.placement.matrix();
        geometry::surface_mesh(&self.shape)
            .iter()
            .filter_map(|piece| {
                let mut ps = piece.mesh.positions.iter();
                let first = *ps.next()?;
                let (min, max) = ps.fold((first, first), |(lo, hi), p| {
                    (
                        [0, 1, 2].map(|i| lo[i].min(p[i])),
                        [0, 1, 2].map(|i| hi[i].max(p[i])),
                    )
                });
                let m = geometry::mul(&place, &piece.transform);
                Some(crate::render3d::lit::box_corners(min, max, &m))
            })
            .collect()
    }

    /// Which side light counts on (`light::Facing`).
    pub fn facing(&self) -> light::Facing {
        if self.rear {
            light::Facing::Both
        } else {
            light::Facing::Front
        }
    }
}

/// A surface's shape, in its own frame (see `geometry`): centred on its
/// origin, its front towards +Z. Metres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SurfaceShape {
    Flat {
        width: f32,
        height: f32,
    },
    /// Part of an upright cylinder: `width` is measured along the curve (an
    /// LED wall's width as built), at most the whole way round.
    Curved {
        width: f32,
        radius: f32,
        height: f32,
        /// Bulging towards the audience (a pillar) rather than wrapped
        /// around them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        convex: bool,
    },
    /// Flat faces, each placed within the surface and showing its own part
    /// of the picture (`geometry::jumbotron`, `geometry::angled_wall`).
    Faceted {
        facets: Vec<Facet>,
    },
    /// A shape a later NewQ wrote that this one doesn't know: kept as it was
    /// written (and written back so when saved), drawn as nothing here.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

impl SurfaceShape {
    /// Whether this NewQ knows the shape (can draw and edit it).
    pub fn is_known(&self) -> bool {
        !matches!(self, SurfaceShape::Unknown(_))
    }
}

impl Default for SurfaceShape {
    /// A 16:9 screen four metres wide.
    fn default() -> Self {
        SurfaceShape::Flat {
            width: 4.0,
            height: 2.25,
        }
    }
}

/// One flat face of a faceted surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Facet {
    pub width: f32,
    pub height: f32,
    /// Within the surface.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// The part of the surface's picture it shows, `[x, y, w, h]` in 0..1.
    #[serde(default = "whole", skip_serializing_if = "is_whole")]
    pub uv: [f32; 4],
}

fn whole() -> [f32; 4] {
    WHOLE
}

/// What a surface shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Feed {
    #[default]
    None,
    /// A display's picture, or the `region` of it (`[x, y, w, h]` in 0..1).
    Display {
        output: OutputId,
        #[serde(default = "whole", skip_serializing_if = "is_whole")]
        region: [f32; 4],
    },
    /// A feed a later NewQ wrote that this one doesn't know: kept as it was
    /// written, showing nothing here.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

impl Feed {
    pub fn is_none(&self) -> bool {
        *self == Feed::None
    }

    /// The display's picture, all of it. For a projector that shows part of
    /// another display's picture (a blended group), that whole picture
    /// instead: what the audience sees on the screen (`Show::whole_picture`).
    pub fn whole_picture(show: &Show, output: OutputId) -> Feed {
        Feed::Display {
            output: show.whole_picture(output),
            region: WHOLE,
        }
    }

    pub fn output(&self) -> Option<OutputId> {
        match self {
            Feed::None | Feed::Unknown(_) => None,
            Feed::Display { output, .. } => Some(*output),
        }
    }
}

/// A display that shows the 3D stage instead of its cues
/// (`OutputConfig::source`): seen from a saved viewpoint, by its name, or
/// with no name (or one no longer there) from in front, with everything in
/// view. Drawn at the display's own size and at full quality: a display is
/// never governed as a preview is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewpoint: Option<String>,
}

/// A saved camera position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewpoint {
    pub name: String,
    pub eye: [f32; 3],
    pub target: [f32; 3],
    #[serde(default = "default_fov", skip_serializing_if = "is_default_fov")]
    pub fov_deg: f32,
}

fn default_fov() -> f32 {
    50.0
}

fn is_default_fov(v: &f32) -> bool {
    *v == default_fov()
}

impl Viewpoint {
    pub fn camera(&self) -> scene::Camera {
        let finite = |p: [f32; 3]| p.iter().all(|v| v.is_finite());
        let mut cam = scene::Camera::default();
        if finite(self.eye) && finite(self.target) && self.eye != self.target {
            cam.eye = self.eye;
            cam.target = self.target;
        }
        if self.fov_deg.is_finite() {
            cam.fov_y_deg = self.fov_deg.clamp(5.0, 150.0);
        }
        cam
    }
}

impl Stage3d {
    pub fn is_empty(&self) -> bool {
        *self == Stage3d::default()
    }

    pub fn surface(&self, id: SurfaceId) -> Option<&Surface> {
        self.surfaces.iter().find(|s| s.id == id)
    }

    pub fn surface_mut(&mut self, id: SurfaceId) -> Option<&mut Surface> {
        self.surfaces.iter_mut().find(|s| s.id == id)
    }

    /// A surface whose display is no longer in the show shows nothing, not a
    /// picture that isn't there (a display removed, a hand-edited file).
    pub fn forget_missing_outputs(&mut self, exists: impl Fn(OutputId) -> bool) {
        for s in &mut self.surfaces {
            if s.feed.output().is_some_and(|o| !exists(o)) {
                s.feed = Feed::None;
            }
        }
        // A projector whose display has gone is a projector with none.
        for p in &mut self.projectors {
            if p.display.is_some_and(|o| !exists(o)) {
                p.display = None;
            }
        }
    }

    /// What the 3D view draws: the venue (lit) if it is loaded and shown,
    /// and every visible surface (unlit), painted with its display's picture,
    /// or dim grey with none.
    pub fn scene(&self, show: &Show, venue: Option<&LoadedVenue>) -> Scene {
        let mut parts = Vec::new();
        if let (Some(v), Some(loaded)) = (&self.venue, venue)
            && v.visible
        {
            let transform = v.matrix();
            parts.extend(loaded.parts.iter().map(|p| Part {
                mesh: p.mesh.clone(),
                mesh_key: p.mesh_key,
                transform,
                paint: p.paint.clone(),
                lit: true,
                catch: Default::default(),
            }));
        }
        // Facets of one size share a mesh.
        let mut meshes: HashMap<u64, Arc<Mesh>> = HashMap::new();
        for s in self.surfaces.iter().filter(|s| s.visible) {
            let place = s.placement.matrix();
            // A display that shows the stage can't be a picture on it: it
            // would be drawn from itself (and sampling the texture being drawn
            // into is an error on the graphics card). It is left unfed.
            let feed = match s.feed {
                Feed::Display { output, region }
                    if show.output(output).is_some() && !show.shows_stage(output) =>
                {
                    Some((output, geometry::region(region)))
                }
                _ => None,
            };
            // An LED screen takes no projector's light (mapping plan 8).
            let led = show.kind_of(s) == ScreenKind::Led;
            for piece in geometry::surface_mesh(&s.shape) {
                let key = piece.mesh.content_key();
                let mesh = meshes
                    .entry(key)
                    .or_insert_with(|| Arc::new(piece.mesh))
                    .clone();
                let paint = match feed {
                    Some((output, region)) => Paint::Output {
                        output,
                        region: geometry::compose_region(region, piece.region),
                    },
                    None => Paint::Colour(UNFED_GREY),
                };
                parts.push(Part {
                    mesh,
                    mesh_key: key,
                    transform: geometry::mul(&place, &piece.transform),
                    paint,
                    lit: false,
                    // A front screen takes light on its front only (`light`).
                    catch: crate::stage3d::scene::Catch {
                        gain: s.gain,
                        front_only: !s.rear,
                        led,
                    },
                });
            }
        }
        // The projectors' bodies, last (`part_projector`), lit.
        if self.projectors.iter().any(|p| p.visible) {
            let body = Arc::new(projector_body());
            let key = body.content_key();
            for p in self.projectors.iter().filter(|p| p.visible) {
                parts.push(Part {
                    mesh: body.clone(),
                    mesh_key: key,
                    transform: p.placement.matrix(),
                    paint: Paint::Colour(PROJECTOR_GREY),
                    lit: true,
                    catch: Default::default(),
                });
            }
        }
        Scene {
            parts,
            throws: Vec::new(),
        }
    }

    /// Where in a `scene` made by `scene` the projectors' bodies begin: the
    /// parts before are the venue and the screens (what light lands on and
    /// what can be in its way).
    pub fn projectors_from(&self, scene: &Scene) -> usize {
        let n = self.projectors.iter().filter(|p| p.visible).count();
        scene.parts.len().saturating_sub(n)
    }

    /// Which projector part `part` of a `scene` made by `scene` is the body
    /// of, if any: for picking one with the mouse.
    pub fn part_projector(&self, scene: &Scene, part: usize) -> Option<ProjectorId> {
        let i = part.checked_sub(self.projectors_from(scene))?;
        self.projectors
            .iter()
            .filter(|p| p.visible)
            .nth(i)
            .map(|p| p.id)
    }

    pub fn projector(&self, id: ProjectorId) -> Option<&Projector> {
        self.projectors.iter().find(|p| p.id == id)
    }

    /// Which surface part `part` of a `scene` made by `scene` belongs to (the
    /// venue's parts, which come first and are lit, belong to none): for
    /// picking a screen with the mouse.
    pub fn part_surface(&self, scene: &Scene, part: usize) -> Option<SurfaceId> {
        let p = scene.parts.get(part)?;
        if p.lit || part >= self.projectors_from(scene) {
            return None;
        }
        let venue_parts = scene.parts.iter().take_while(|p| p.lit).count();
        let mut i = part.checked_sub(venue_parts)?;
        for s in self.surfaces.iter().filter(|s| s.visible) {
            let n = geometry::surface_mesh(&s.shape).len();
            if i < n {
                return Some(s.id);
            }
            i -= n;
        }
        None
    }

    /// A sample venue's viewpoints (`samples::viewpoints`) placed where the
    /// venue is, leaving out any whose name is already saved.
    pub fn add_sample_viewpoints(&mut self, kind: samples::SampleVenue) {
        let m = self
            .venue
            .as_ref()
            .map_or(scene::IDENTITY, |v| v.placement.matrix());
        for v in samples::viewpoints(kind) {
            if self.viewpoints.iter().any(|w| w.name == v.name) {
                continue;
            }
            self.viewpoints.push(Viewpoint {
                eye: geometry::point(&m, v.eye),
                target: geometry::point(&m, v.target),
                ..v
            });
        }
    }

    /// The venue's corners in the world, once loaded: to frame it in view.
    pub fn venue_bounds(&self, loaded: &LoadedVenue) -> Option<load::Bounds> {
        let v = self.venue.as_ref()?;
        let m = v.matrix();
        let b = loaded.bounds?;
        let mut out: Option<load::Bounds> = None;
        for i in 0..8 {
            let c = [
                if i & 1 == 0 { b.min[0] } else { b.max[0] },
                if i & 2 == 0 { b.min[1] } else { b.max[1] },
                if i & 4 == 0 { b.min[2] } else { b.max[2] },
            ];
            let p = geometry::point(&m, c);
            out = Some(match out {
                None => load::Bounds { min: p, max: p },
                Some(o) => o.with(p),
            });
        }
        out
    }

    /// A key that changes when anything the scene is made from changes but
    /// the pictures: for a caller that keeps a `Scene` between frames.
    /// (Written straight into the hash, never as a string: the app works it
    /// out only when the stage is no longer equal to the one it kept.)
    pub fn shape_key(&self) -> u64 {
        struct Hashing(Fnv);
        impl std::io::Write for Hashing {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.bytes(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut h = Hashing(Fnv::new());
        // A projector's fine-tune and line-up points change no part of the
        // scene (nor a lens map): left out, so dragging one rebuilds nothing.
        if self
            .projectors
            .iter()
            .any(|p| p.fine.is_some() || p.line_up.is_some())
        {
            let still = Stage3d {
                projectors: self.projectors.iter().map(|p| p.without_tuning()).collect(),
                ..self.clone()
            };
            let _ = serde_json::to_writer(&mut h, &still);
        } else {
            let _ = serde_json::to_writer(&mut h, self);
        }
        h.0.0
    }
}

/// The displays whose pictures `scene` shows (each once, in the order first
/// met): the pictures to have ready before it is drawn.
pub fn outputs_in(scene: &Scene) -> Vec<OutputId> {
    let mut out = Vec::new();
    for p in &scene.parts {
        if let Paint::Output { output, .. } = p.paint
            && !out.contains(&output)
        {
            out.push(output);
        }
    }
    out
}

impl Show {
    /// Whether display `output` shows the 3D stage, itself or as the
    /// display whose picture it shows a part of.
    pub fn shows_stage(&self, output: OutputId) -> bool {
        self.stage_source(output).is_some()
    }

    /// The displays that are canvases (`is_canvas`), in the show's order.
    pub fn canvases(&self) -> Vec<OutputId> {
        self.outputs
            .iter()
            .map(|o| o.id)
            .filter(|o| self.is_canvas(*o))
            .collect()
    }

    /// Whether display `output` is a canvas (worked out, never stored): a
    /// whole display (not a sub display, not a part of another's picture,
    /// not one that shows the stage, not a mapped projector's), with no
    /// monitor, that at least one screen shows.
    pub fn is_canvas(&self, output: OutputId) -> bool {
        let Some(o) = self.output(output) else {
            return false;
        };
        !o.is_sub_display()
            && o.part_of.is_none()
            && o.source.is_none()
            && o.monitor.is_none()
            && self.mapped_projector(output).is_none()
            && self
                .stage3d
                .surfaces
                .iter()
                .any(|s| s.feed.output() == Some(output))
    }

    /// Whether screen `surface` can move (a screen cue names it; later, a
    /// tracker bound to it): worked out from the show, armed or not, so it
    /// changes only when the show is edited. A movable screen is never in a
    /// lens map; it is drawn live (mapping plan 4.2). For many screens at
    /// once, `Show::movable_screens` looks once.
    pub fn movable(&self, surface: SurfaceId) -> bool {
        self.cues.iter().any(
            |c| matches!(&c.kind, crate::show::CueKind::Screen(s) if s.screen == Some(surface)),
        )
    }

    /// How display `output`'s picture shows the stage, if it does: the
    /// source of the display whose whole picture it shows (itself, or the one
    /// it shows part of). A sub display never shows the stage (it is an area
    /// of a display's picture; `sanitize_stage` clears one written by hand).
    pub fn stage_source(&self, output: OutputId) -> Option<&StageSource> {
        let whole = self.whole_picture(output);
        let c = self.output(whole)?;
        if c.is_sub_display() {
            return None;
        }
        c.source.as_ref()
    }

    /// Whether display `output` may be given a stage source of its own: a
    /// display that shows part of another's picture shows that one's, and a
    /// sub display is an area of another's (None: it may; Some: why not).
    pub fn stage_source_refused(&self, output: OutputId) -> Option<&'static str> {
        let c = self.output(output)?;
        if c.is_sub_display() {
            Some("A sub display is an area of another display's picture: it can't show the stage.")
        } else if self.mapped_projector(output).is_some() {
            Some(MAPPED_SHOWS_NO_STAGE)
        } else if self.part_of(output).is_some() {
            Some(
                "This display shows part of another display's picture: set the stage on that \
                 display, and this one shows its part.",
            )
        } else {
            None
        }
    }

    /// Rename viewpoint `index` to `name`, and every display that shows the
    /// stage from it with it (in the one edit, so one undo step): a display's
    /// source names its viewpoint. Refused (false, nothing changed) for an
    /// empty name or one another viewpoint has, so two viewpoints never
    /// share a name and a display never moves to another's.
    pub fn rename_viewpoint(&mut self, index: usize, name: &str) -> bool {
        let name = name.trim();
        let taken = self
            .stage3d
            .viewpoints
            .iter()
            .enumerate()
            .any(|(i, v)| i != index && v.name == name);
        let Some(v) = self.stage3d.viewpoints.get_mut(index) else {
            return false;
        };
        if name.is_empty() || taken {
            return false;
        }
        if v.name == name {
            return true;
        }
        let old = std::mem::replace(&mut v.name, name.to_string());
        for o in &mut self.outputs {
            if let Some(s) = &mut o.source
                && s.viewpoint.as_deref() == Some(old.as_str())
            {
                s.viewpoint = Some(name.to_string());
            }
        }
        true
    }

    /// Put right what a hand-edited (or a later NewQ's) file may say: a sub
    /// display that shows the stage (it would draw a picture within itself).
    /// Two viewpoints of one name (the second is named again: a display's
    /// source names the first).
    pub fn sanitize_stage(&mut self) {
        self.sanitize_projectors();
        // A mapped projector's display is never on a screen: it shows the
        // screens, so it would show itself. (The page never lets one be.)
        let mapped: Vec<OutputId> = self
            .stage3d
            .projectors
            .iter()
            .filter(|p| p.mapped)
            .filter_map(|p| p.display)
            .collect();
        for s in &mut self.stage3d.surfaces {
            if s.feed.output().is_some_and(|o| mapped.contains(&o)) {
                log::warn!(
                    "3D stage: screen {} showed a mapped projector's display; it shows nothing",
                    s.name
                );
                s.feed = Feed::None;
            }
        }
        // (`is_sub_display` reads only the display itself.)
        for o in &mut self.outputs {
            if o.is_sub_display() && o.source.take().is_some() {
                log::warn!("3D stage: the sub display {} can't show the stage", o.name);
            }
        }
        let mut seen: Vec<String> = Vec::new();
        for v in &mut self.stage3d.viewpoints {
            if seen.contains(&v.name) {
                let mut n = 2;
                while seen.contains(&format!("{} {n}", v.name)) {
                    n += 1;
                }
                v.name = format!("{} {n}", v.name);
            }
            seen.push(v.name.clone());
        }
    }

    /// The camera a display showing the stage sees it with: its viewpoint,
    /// or with none (or one not there) from in front of `bounds` (the
    /// stage's, if known), the whole of it in view.
    pub fn stage_camera(
        &self,
        source: &StageSource,
        bounds: Option<([f32; 3], [f32; 3])>,
        aspect: f32,
    ) -> scene::Camera {
        if let Some(name) = &source.viewpoint
            && let Some(v) = self.stage3d.viewpoints.iter().find(|v| &v.name == name)
        {
            return v.camera();
        }
        let Some((min, max)) = bounds else {
            return scene::Camera::default();
        };
        let centre = [0, 1, 2].map(|i| (min[i] + max[i]) * 0.5);
        let radius = (0..3)
            .map(|i| (max[i] - min[i]) * 0.5)
            .map(|h| h * h)
            .sum::<f32>()
            .sqrt()
            .max(0.5);
        let mut cam = scene::Camera::default();
        let half_y = (cam.fov_y_deg.to_radians() * 0.5).max(0.01);
        let aspect = if aspect.is_finite() && aspect > 0.01 {
            aspect
        } else {
            16.0 / 9.0
        };
        let half = half_y.min((half_y.tan() * aspect).atan());
        let distance = radius * 1.05 / half.sin();
        // A little above, as from the back of the stalls.
        let (s, c) = 12f32.to_radians().sin_cos();
        cam.target = centre;
        cam.eye = [
            centre[0],
            centre[1] + distance * s,
            centre[2] + distance * c,
        ];
        cam.far = cam.far.max(distance * 100.0);
        cam
    }

    /// The display whose picture `output` shows: itself, or, for a display
    /// that shows part of another's picture (a projector of a blended group,
    /// a mirror), that other display.
    pub fn whole_picture(&self, output: OutputId) -> OutputId {
        self.part_of(output).map_or(output, |p| p.display)
    }

    /// Add a surface (a flat 16:9 screen if `shape` is None) showing nothing
    /// yet, named "Screen N".
    pub fn add_surface(&mut self, shape: Option<SurfaceShape>) -> SurfaceId {
        let id = SurfaceId(self.alloc_id());
        let n = self.stage3d.surfaces.len() + 1;
        self.stage3d.surfaces.push(Surface {
            id,
            name: format!("Screen {n}"),
            visible: true,
            placement: Placement::default(),
            shape: shape.unwrap_or_default(),
            feed: Feed::None,
            gain: 1.0,
            rear: false,
            kind: None,
            extra: Default::default(),
        });
        id
    }

    /// Add a sample venue's usual screens (`samples::screens`), unfed, where
    /// they belong in it: the venue's position and turn (yaw) are followed;
    /// a venue tipped or rolled isn't. Ids from the show's.
    pub fn add_sample_screens(&mut self, kind: samples::SampleVenue) -> Vec<SurfaceId> {
        let venue = self
            .stage3d
            .venue
            .as_ref()
            .map_or_else(Placement::default, |v| v.placement);
        let turn = geometry::rotation_y(if venue.yaw.is_finite() {
            venue.yaw
        } else {
            0.0
        });
        let mut ids = Vec::new();
        for s in samples::screens(kind) {
            let id = SurfaceId(self.alloc_id());
            let p = geometry::point(&turn, s.placement.position);
            self.stage3d.surfaces.push(Surface {
                id,
                name: s.name.to_string(),
                visible: true,
                placement: Placement {
                    position: [0, 1, 2].map(|i| p[i] + venue.position[i]),
                    yaw: s.placement.yaw + venue.yaw,
                    ..s.placement
                },
                shape: s.shape,
                feed: Feed::None,
                gain: 1.0,
                rear: false,
                kind: None,
                extra: Default::default(),
            });
            ids.push(id);
        }
        ids
    }

    pub fn remove_surface(&mut self, id: SurfaceId) {
        self.stage3d.surfaces.retain(|s| s.id != id);
    }

    /// After the displays changed: no surface shows one that has gone.
    pub fn forget_missing_feeds(&mut self) {
        let outputs: Vec<OutputId> = self.outputs.iter().map(|o| o.id).collect();
        self.stage3d
            .forget_missing_outputs(|o| outputs.contains(&o));
    }
}

#[cfg(test)]
mod tests;
