//! The neutral scene types the renderer draws. A copy of `stage3d::scene`'s
//! (written by the model's side at the same time); the two are to be made one.
//!
//! The world is right-handed, Y up, in metres. Conventions the renderer
//! takes: a mesh's uv (0, 0) is the picture's top-left and v runs down (as in
//! glTF and wgpu); `Paint::Output::region` is (left, top, width, height) as
//! fractions of the picture, like `show::PartOf::area`; a triangle's winding
//! doesn't matter (nothing is culled).

use crate::show::OutputId;
use std::sync::Arc;

pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

pub enum Paint {
    Colour([f32; 4]),
    Image {
        rgba: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        key: u64,
    },
    Output {
        output: OutputId,
        region: [f32; 4],
    },
}

pub struct Part {
    pub mesh: Arc<Mesh>,
    pub mesh_key: u64,
    /// Column-major, model to world.
    pub transform: [[f32; 4]; 4],
    pub paint: Paint,
    pub lit: bool,
}

pub struct Camera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
}

pub struct Scene {
    pub parts: Vec<Part>,
}
