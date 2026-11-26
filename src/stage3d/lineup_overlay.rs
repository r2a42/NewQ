//! Drawing a line-up's crosses on a projector (mapping plan 4.4), pure: the
//! little sprite of each cross and its number, where a sprite goes, and the
//! model's outline as a whole-screen picture.
//!
//! How the app uses them (`app.rs`, the overlay's group in
//! `update_outputs`): while a line-up overlay is held for a display, its
//! monitor's window is sent black (or its usual list dimmed) and then one
//! last group whose canvas is the monitor's own size in raw pixels (no
//! keystone, mesh, map or fine-tune; the window's edge blend off). In that
//! group: the outline picture first (`outline_picture`, made at the
//! monitor's size, `Fit::Stretch`, drawn again only when a new outline
//! comes), then one layer per mark: its sprite (`sprite`, one texture per
//! number and look, made once) with `Fit::Stretch` and `sprite_transform`.
//! So a nudge changes a few numbers and uploads nothing.
//!
//! (Round 2, agent A: the drawing. The contract's bodies are plain
//! stand-ins: a cross with no number, and no outline.)

#![allow(dead_code)]

use crate::media::video::RgbaImage;
use crate::show::LayerTransform;

/// A sprite's side in pixels (square).
pub const SPRITE: u32 = 64;

/// How a cross is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Plain,
    /// The point the operator is moving.
    Active,
    /// A point switched off: dim.
    Off,
}

/// The cross for point `n` (its number as the operator sees it), with its
/// ring and number, `SPRITE` square, its middle the point, transparent
/// around.
pub fn sprite(n: u16, look: Look) -> RgbaImage {
    let _ = n;
    let side = SPRITE as usize;
    let mut data = vec![0u8; side * side * 4];
    let v = match look {
        Look::Plain => [255, 255, 255, 255],
        Look::Active => [255, 220, 0, 255],
        Look::Off => [110, 110, 110, 255],
    };
    for i in 0..side {
        for (x, y) in [(i, side / 2), (side / 2, i)] {
            let k = (y * side + x) * 4;
            data[k..k + 4].copy_from_slice(&v);
        }
    }
    RgbaImage {
        width: SPRITE,
        height: SPRITE,
        data,
    }
}

/// Where a sprite goes: its layer is `Fit::Stretch` in a group whose canvas
/// is `monitor` (raw pixels), so this puts its middle at `at` (pixels in a
/// picture `pixels` big, scaled onto the monitor) at `SPRITE` monitor pixels
/// square. A picture or monitor with no size gives a sprite out of sight.
pub fn sprite_transform(at: [f32; 2], pixels: (u32, u32), monitor: (u32, u32)) -> LayerTransform {
    if pixels.0 == 0 || pixels.1 == 0 || monitor.0 == 0 || monitor.1 == 0 {
        return LayerTransform {
            scale_x: 0.0,
            scale_y: 0.0,
            ..LayerTransform::default()
        };
    }
    LayerTransform {
        x: at[0] / pixels.0 as f32 - 0.5,
        y: at[1] / pixels.1 as f32 - 0.5,
        scale_x: SPRITE as f32 / monitor.0 as f32,
        scale_y: SPRITE as f32 / monitor.1 as f32,
        ..LayerTransform::default()
    }
}

/// The model's outline (pieces of lines, in pixels of a picture `pixels`
/// big) drawn into a picture `size` big (the monitor's), transparent
/// elsewhere.
pub fn outline_picture(
    outline: &[Vec<[f32; 2]>],
    pixels: (u32, u32),
    size: (u32, u32),
) -> RgbaImage {
    let _ = (outline, pixels);
    RgbaImage {
        width: size.0.max(1),
        height: size.1.max(1),
        data: vec![0; size.0.max(1) as usize * size.1.max(1) as usize * 4],
    }
}
