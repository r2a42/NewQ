//! The neutral scene types the renderer draws: the 3D stage's own
//! (`stage3d::scene`), whose header gives the conventions. The renderer takes
//! a mesh's uv (0, 0) as the picture's top-left with v running down,
//! `Paint::Output::region` as (left, top, width, height) fractions like
//! `show::PartOf::area`, and draws both faces (nothing is culled).

pub use crate::stage3d::scene::*;
