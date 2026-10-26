//! NewQ's logo as the app uses it: the window icon and the About window's picture.
//!
//! The artwork is `assets/logo/` (made by `scripts/make_logo_assets.py`); this
//! embeds the 128 × 128 picture as raw RGBA, so no image decoder is needed.

/// Side of the embedded icon, in pixels.
pub const ICON_SIDE: u32 = 128;

/// The icon as straight (not premultiplied) RGBA, row by row.
pub const ICON_RGBA: &[u8] = include_bytes!("../assets/logo/newq-128.rgba");

/// The icon for a window's title bar and the taskbar (Windows, and X11 and
/// Wayland desktops; a Mac takes the Dock's from the app bundle).
pub fn window_icon() -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIDE, ICON_SIDE).ok()
}

/// The icon as a picture for egui.
pub fn icon_image() -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([ICON_SIDE as usize; 2], ICON_RGBA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_is_a_whole_picture_with_rounded_corners() {
        assert_eq!(ICON_RGBA.len(), (ICON_SIDE * ICON_SIDE * 4) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * ICON_SIDE + x) * 4) as usize;
            [
                ICON_RGBA[i],
                ICON_RGBA[i + 1],
                ICON_RGBA[i + 2],
                ICON_RGBA[i + 3],
            ]
        };
        assert_eq!(at(0, 0)[3], 0, "the corner of the tile is clear");
        assert_eq!(at(ICON_SIDE - 1, 0)[3], 0);
        let edge = at(ICON_SIDE / 2, 4);
        assert_eq!(edge[3], 255, "the tile is solid inside its corners");
        assert!(edge[0] < 40 && edge[1] < 40, "and dark: {edge:?}");
        // Some of it is the ivory ring.
        let light = (0..ICON_SIDE * ICON_SIDE)
            .filter(|i| {
                ICON_RGBA[(*i * 4) as usize] > 230 && ICON_RGBA[(*i * 4 + 3) as usize] == 255
            })
            .count();
        assert!(light > 500, "{light} ivory pixels");
        assert!(window_icon().is_some());
        assert_eq!(icon_image().size, [128, 128]);
    }
}
