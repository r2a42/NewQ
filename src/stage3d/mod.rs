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
pub mod load;
pub mod scene;

use crate::show::{OutputId, Show};
use geometry::Mat4;
use load::LoadedVenue;
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
    /// How lengths are shown to the user. Kept in metres whatever this says.
    #[serde(default, skip_serializing_if = "LengthUnit::is_default")]
    pub units: LengthUnit,
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

/// The venue: a model file of the room, stage or arena.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Venue {
    /// The model file (.glb, .gltf or .obj). Kept relative to the show file
    /// when saved, as cues' files are.
    pub path: PathBuf,
    /// Where the model is put, after its units and up axis.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// What the file's numbers are in (OBJ has no units; glTF is metres).
    #[serde(default, skip_serializing_if = "LengthUnit::is_default")]
    pub units: LengthUnit,
    #[serde(default, skip_serializing_if = "UpAxis::is_default")]
    pub up: UpAxis,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Its colours and pictures multiplied by this (0.05 to 4), to dim a
    /// bright model so the screens stand out. Applied when the model is
    /// loaded (`load_venue`): a change loads it again (`needs_reload`).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub brightness: f32,
}

impl Venue {
    pub fn new(path: PathBuf) -> Self {
        Venue {
            path,
            placement: Placement::default(),
            units: LengthUnit::Metres,
            up: UpAxis::Y,
            visible: true,
            brightness: 1.0,
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
        self.path != loaded_for.path || self.brightness() != loaded_for.brightness()
    }

    /// The model's file coordinates to the world.
    pub fn matrix(&self) -> Mat4 {
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
            Feed::None => None,
            Feed::Display { output, .. } => Some(*output),
        }
    }
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
            }));
        }
        // Facets of one size share a mesh.
        let mut meshes: HashMap<u64, Arc<Mesh>> = HashMap::new();
        for s in self.surfaces.iter().filter(|s| s.visible) {
            let place = s.placement.matrix();
            let feed = match s.feed {
                Feed::Display { output, region } if show.output(output).is_some() => {
                    Some((output, geometry::region(region)))
                }
                _ => None,
            };
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
                });
            }
        }
        Scene { parts }
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
    pub fn shape_key(&self) -> u64 {
        let mut h = Fnv::new();
        h.bytes(serde_json::to_string(self).unwrap_or_default().as_bytes());
        h.0
    }
}

impl Show {
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
        });
        id
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
