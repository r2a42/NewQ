//! The splash screen: a small window with the logo and the version, shown the
//! moment the program starts while the main window is made ready (the graphics
//! card is set up first, which takes a moment), and dismissed by itself.
//!
//! It is drawn on the processor into a plain pixel buffer (`softbuffer`), so it
//! doesn't wait for the graphics card, which is what it covers for. `compose`
//! makes the picture and knows nothing about windows, so it is tested.

use crate::show::{TextCue, TextVAlign};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

/// The size of the window, in logical pixels.
pub const WIDTH: f64 = 460.0;
pub const HEIGHT: f64 = 300.0;
/// How long it stays, at least: long enough to read the version.
pub const HOLD: Duration = Duration::from_millis(2500);
/// How long it may stay if the main window is slow: it never stays for ever.
pub const CAP: Duration = Duration::from_secs(10);

const BACKGROUND: [u8; 3] = [20, 22, 27];
const BORDER: [u8; 3] = [52, 57, 68];
const TITLE: [u8; 3] = [240, 242, 246];
const VERSION: [u8; 3] = [168, 176, 192];
const TAGLINE: [u8; 3] = [122, 129, 144];

fn pack(rgb: [u8; 3]) -> u32 {
    u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2])
}

/// Lay `rgb` over the pixel with opacity `a` (0 to 1).
fn blend(px: &mut u32, rgb: [u8; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let a = a.min(1.0);
    let channel = |shift: u32, new: u8| {
        let old = ((*px >> shift) & 0xff) as f32;
        (old + (f32::from(new) - old) * a).round() as u32
    };
    *px = channel(16, rgb[0]) << 16 | channel(8, rgb[1]) << 8 | channel(0, rgb[2]);
}

/// The picture for a window of `w` x `h` pixels, as 0x00RRGGBB pixels. Sizes
/// are laid out for `WIDTH` x `HEIGHT` and scaled with the window.
pub fn compose(w: u32, h: u32, version: &str) -> Vec<u32> {
    let (w, h) = (w.max(2), h.max(2));
    let scale = w as f32 / WIDTH as f32;
    let mut px = vec![pack(BACKGROUND); (w * h) as usize];

    // A frame, so the window has an edge against whatever is behind it.
    let edge = scale.round().max(1.0) as u32;
    for y in 0..h {
        for x in 0..w {
            if x < edge || y < edge || x >= w - edge || y >= h - edge {
                px[(y * w + x) as usize] = pack(BORDER);
            }
        }
    }

    // The logo, centred near the top.
    let side = (112.0 * scale).round().max(2.0) as u32;
    draw_logo(
        &mut px,
        w,
        h,
        (w.saturating_sub(side)) / 2,
        (28.0 * scale) as u32,
        side,
    );

    // Name, version and what it is, each in a band of its own.
    let band = |px: &mut Vec<u32>, text: &str, top: f32, height: f32, size: f32, rgb, bold| {
        draw_text(
            px,
            w,
            (top * scale) as u32,
            ((height * scale) as u32).max(2),
            text,
            size,
            rgb,
            bold,
        );
    };
    band(&mut px, "NewQ", 146.0, 66.0, 0.72, TITLE, true);
    band(
        &mut px,
        &format!("Version {version}"),
        212.0,
        30.0,
        0.62,
        VERSION,
        false,
    );
    band(
        &mut px,
        "Cue-based playback for theatre and live events",
        248.0,
        26.0,
        0.55,
        TAGLINE,
        false,
    );
    px
}

/// The 128 px logo, scaled to `side` pixels with its own transparent corners.
fn draw_logo(px: &mut [u32], w: u32, h: u32, x0: u32, y0: u32, side: u32) {
    let src = crate::branding::ICON_RGBA;
    let n = crate::branding::ICON_SIDE as usize;
    let at = |x: usize, y: usize| {
        let i = (y * n + x) * 4;
        [
            f32::from(src[i]),
            f32::from(src[i + 1]),
            f32::from(src[i + 2]),
            f32::from(src[i + 3]),
        ]
    };
    for dy in 0..side {
        for dx in 0..side {
            let (x, y) = (x0 + dx, y0 + dy);
            if x >= w || y >= h {
                continue;
            }
            // Bilinear, so the logo is smooth at the window's own size.
            let fx = (dx as f32 + 0.5) * n as f32 / side as f32 - 0.5;
            let fy = (dy as f32 + 0.5) * n as f32 / side as f32 - 0.5;
            let (ix, iy) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (jx, jy) = ((ix + 1).min(n - 1), (iy + 1).min(n - 1));
            let (tx, ty) = (
                (fx - ix as f32).clamp(0.0, 1.0),
                (fy - iy as f32).clamp(0.0, 1.0),
            );
            let (a, b, c, d) = (at(ix, iy), at(jx, iy), at(ix, jy), at(jx, jy));
            let mix = |k: usize| {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bottom = c[k] + (d[k] - c[k]) * tx;
                top + (bottom - top) * ty
            };
            let rgb = [
                mix(0).round() as u8,
                mix(1).round() as u8,
                mix(2).round() as u8,
            ];
            blend(&mut px[(y * w + x) as usize], rgb, mix(3) / 255.0);
        }
    }
}

/// One line of text, centred across the window, in a band `height` pixels high
/// starting at row `top`; `size` is the letters' height as a part of the band.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    px: &mut [u32],
    w: u32,
    top: u32,
    height: u32,
    text: &str,
    size: f32,
    rgb: [u8; 3],
    bold: bool,
) {
    let mut cue = TextCue::new(None);
    cue.text = text.to_string();
    cue.size = size;
    cue.valign = TextVAlign::Middle;
    cue.width = 0.96;
    cue.shadow = false;
    cue.bold = bold;
    let img = crate::text::render(&cue, (w, height));
    let rows = px.len() as u32 / w;
    for y in 0..height {
        if top + y >= rows {
            break;
        }
        for x in 0..w {
            let a = f32::from(img.data[((y * w + x) * 4 + 3) as usize]) / 255.0;
            blend(&mut px[((top + y) * w + x) as usize], rgb, a);
        }
    }
}

/// Whether the splash screen has been up long enough to go: `HOLD` has passed
/// and the main window is there to take over, or `CAP` has, whatever happens.
pub fn due(opened: Instant, now: Instant, main_shown: bool) -> bool {
    let up = now.saturating_duration_since(opened);
    (up >= HOLD && main_shown) || up >= CAP
}

/// The window and what it draws on.
pub struct Splash {
    window: Arc<Window>,
    surface: softbuffer::Surface<winit::event_loop::OwnedDisplayHandle, Arc<Window>>,
    pub opened: Instant,
}

impl Splash {
    /// Open and draw it, centred on the main display. `None` if the system
    /// won't make a window like this (the program starts without one).
    pub fn open(event_loop: &ActiveEventLoop) -> Option<Splash> {
        let mut attrs = Window::default_attributes()
            .with_title("NewQ")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_icon(crate::branding::window_icon())
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT));
        if let Some(m) = event_loop.primary_monitor() {
            let s = m.scale_factor();
            let (ww, wh) = (WIDTH * s, HEIGHT * s);
            let (mp, ms) = (m.position(), m.size());
            attrs = attrs.with_position(PhysicalPosition::new(
                mp.x + ((f64::from(ms.width) - ww) / 2.0).max(0.0) as i32,
                mp.y + ((f64::from(ms.height) - wh) / 2.0).max(0.0) as i32,
            ));
        }
        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        let context = softbuffer::Context::new(event_loop.owned_display_handle()).ok()?;
        let surface = softbuffer::Surface::new(&context, window.clone()).ok()?;
        let mut splash = Splash {
            window,
            surface,
            opened: Instant::now(),
        };
        splash.paint();
        Some(splash)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Draw the picture (again).
    pub fn paint(&mut self) {
        let size = self.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if self.surface.resize(w, h).is_err() {
            return;
        }
        let picture = compose(size.width, size.height, env!("CARGO_PKG_VERSION"));
        if let Ok(mut buffer) = self.surface.buffer_mut() {
            let n = buffer.len().min(picture.len());
            buffer[..n].copy_from_slice(&picture[..n]);
            let _ = buffer.present();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(p: &[u32], w: u32, x: u32, y: u32) -> [u8; 3] {
        let v = p[(y * w + x) as usize];
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    #[test]
    fn the_picture_has_the_frame_the_logo_and_the_words() {
        let (w, h) = (WIDTH as u32, HEIGHT as u32);
        let p = compose(w, h, "9.9.9");
        assert_eq!(p.len(), (w * h) as usize);
        assert_eq!(at(&p, w, 0, 0), BORDER, "an edge to the window");
        assert_eq!(at(&p, w, 10, 10), BACKGROUND);
        // The logo is in the middle of the top part: not the background.
        assert_ne!(at(&p, w, w / 2, 84), BACKGROUND);
        // The words are lighter than the background somewhere in their bands.
        let lit =
            |top: u32, bottom: u32| (top..bottom).any(|y| (0..w).any(|x| at(&p, w, x, y)[0] > 100));
        assert!(lit(150, 212), "the name");
        assert!(lit(214, 242), "the version");
        assert!(lit(250, 274), "the tagline");
    }

    #[test]
    fn the_version_in_the_picture_is_the_one_asked_for() {
        let (w, h) = (WIDTH as u32, HEIGHT as u32);
        let a = compose(w, h, "1.2.3");
        let b = compose(w, h, "1.2.4");
        let c = compose(w, h, "1.2.3");
        assert_eq!(a, c);
        assert_ne!(a, b);
        // Only the version band differs.
        let differs = |y: u32| (0..w).any(|x| a[(y * w + x) as usize] != b[(y * w + x) as usize]);
        assert!(!(0..200).any(differs));
        assert!((212..242).any(differs));
    }

    #[test]
    fn a_larger_window_gets_a_picture_of_the_same_shape() {
        // A display with twice the pixels (a Retina Mac).
        let (w, h) = (WIDTH as u32 * 2, HEIGHT as u32 * 2);
        let p = compose(w, h, "1.0.0");
        assert_eq!(p.len(), (w * h) as usize);
        assert_ne!(
            at(&p, w, w / 2, 168),
            BACKGROUND,
            "the logo, at twice the size"
        );
        assert_eq!(at(&p, w, 0, 0), BORDER);
        // Odd or tiny sizes don't fail.
        assert_eq!(compose(1, 1, "x").len(), 4);
    }

    #[test]
    fn it_stays_long_enough_to_be_read_and_never_for_ever() {
        let t = Instant::now();
        let later = |ms: u64| t + Duration::from_millis(ms);
        assert!(
            !due(t, later(100), true),
            "not at once, even with the main window up"
        );
        assert!(!due(t, later(2400), true));
        assert!(
            due(t, later(2500), true),
            "the hold is over and the main window is there"
        );
        assert!(!due(t, later(5000), false), "waits for a slow main window");
        assert!(due(t, later(10_000), false), "but not for ever");
    }
}
