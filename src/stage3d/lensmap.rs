//! The lens map worked out on the CPU (mapping plan 4.1), pure: for each
//! pixel of a mapped projector's picture, the ray through the lens
//! (`Projector::ray`), the nearest still screen it meets in the scene
//! (`render3d::pick`, the venue in the way), and that spot's place in its
//! feed's picture, packed as `mapped::pack_place`/`pack_how` do. The GPU
//! bake (`render3d::bake`) is tested against it.

#![allow(dead_code)]

use super::projector::Projector;
use crate::mapped::CpuMap;
use crate::render3d::Scene;

/// What projector `projector`'s pixels see of `scene`'s still screens, for a
/// picture `size` big. (Round 1, agent A: the body.)
pub fn reference(_scene: &Scene, _projector: &Projector, size: (u32, u32)) -> CpuMap {
    CpuMap::black(size)
}
