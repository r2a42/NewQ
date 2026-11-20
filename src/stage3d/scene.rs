//! The neutral types the 3D renderer (`render3d`) draws: plain meshes, what
//! paints them, where they are and the camera. Nothing here knows about wgpu.
//!
//! Conventions, the same everywhere a `Scene` is made or drawn:
//! - The world is right-handed, Y up, in metres.
//! - A triangle whose corners go counter-clockwise, seen from in front, faces
//!   you (front faces are CCW). Normals point out of the front.
//! - UVs: (0, 0) is the top-left of a picture, (1, 1) the bottom-right
//!   (v grows downwards, as image rows do).
//! - A `region` is `[x, y, w, h]` in 0..1 of the picture, the same as
//!   `show::PartOf::area`: the part of the picture a mesh's 0..1 UVs cover.
//!   The renderer applies it: `picture_uv = region.xy + uv * region.zw`.
//! - `Part::transform` is column-major: `transform[col][row]`, so the
//!   translation is `transform[3][0..3]`.

// The renderer and the Preview pane's 3D mode use these (integration).
#![allow(dead_code)]

use crate::show::OutputId;
use std::sync::Arc;

/// Triangles, indexed. `positions`, `normals` and `uvs` have one entry per
/// vertex; `indices` three per triangle, CCW at the front.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// A hash of the mesh's contents, for `Part::mesh_key`.
    pub fn content_key(&self) -> u64 {
        let mut h = Fnv::new();
        for p in &self.positions {
            p.iter().for_each(|v| h.f32(*v));
        }
        for n in &self.normals {
            n.iter().for_each(|v| h.f32(*v));
        }
        for t in &self.uvs {
            t.iter().for_each(|v| h.f32(*v));
        }
        for i in &self.indices {
            h.bytes(&i.to_le_bytes());
        }
        h.0
    }
}

/// What a part is painted with.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Linear RGBA, 0..1 (premultiplied alpha is not assumed: a = 1 is opaque).
    Colour([f32; 4]),
    /// A picture, `width * height * 4` bytes of sRGB RGBA, rows top first.
    /// `key` is a hash of its contents (or a stable id), for caching.
    Image {
        rgba: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        key: u64,
    },
    /// A display's picture as it is now, or the `region` of it (see the
    /// module's conventions). The renderer is given the displays' pictures; one
    /// it doesn't have is drawn as it would draw a missing texture.
    Output { output: OutputId, region: [f32; 4] },
}

/// A mesh placed in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub mesh: Arc<Mesh>,
    /// Stable while the mesh is the same, for caching its upload.
    pub mesh_key: u64,
    /// Model to world, metres, Y up; column-major (`transform[col][row]`).
    pub transform: [[f32; 4]; 4],
    pub paint: Paint,
    /// Shaded by the light (a venue), or drawn at full brightness (a screen).
    pub lit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    /// The picture's height as an angle, top to bottom. With a `shift`, the
    /// picture is this tall but off the view's axis.
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
    /// How far the picture's middle is off the view's axis, as a share of
    /// its width (x, right) and height (y, up): a projector's lens shift
    /// (0.5 up puts the picture's bottom edge on the axis). 0 for an
    /// ordinary camera.
    pub shift: [f32; 2],
    /// A picture shape of its own, width per height (a projector's): drawn
    /// into a target of another shape, the view is widened (or made taller)
    /// round it, so the whole picture is seen and nothing is stretched.
    /// None: the target's shape.
    pub aspect: Option<f32>,
}

impl Default for Camera {
    /// Standing 10 m back from the origin at eye height, looking at it.
    fn default() -> Self {
        Camera {
            eye: [0.0, 1.7, 10.0],
            target: [0.0, 1.7, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 50.0,
            near: 0.05,
            far: 2000.0,
            shift: [0.0; 2],
            aspect: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub parts: Vec<Part>,
}

/// The identity matrix, column-major.
pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// FNV-1a, 64 bits: a stable content hash (`std`'s hasher may change between
/// releases, and these keys only need to tell meshes and pictures apart).
pub(crate) struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn f32(&mut self, v: f32) {
        self.bytes(&v.to_bits().to_le_bytes());
    }
}
