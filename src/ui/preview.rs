//! Where a display's picture goes in a preview (the Preview pane, the
//! workspace's monitor, a sub display's area editor): always the display's own
//! shape, as large as fits, with black bars on the sides or above and below.
//!
//! The picture was once drawn squeezed: the preview's rect ran past the edge
//! of the window (a narrow pane at the window's right edge widened by its own
//! header row, or a tall picture below the bottom), and egui clamps a paint
//! callback's viewport to the window, so the whole picture was drawn into the
//! part left. `space` keeps the rect to what is in sight and `WholeViewport`
//! draws through the unclamped viewport when it still runs over.

use egui::{Rect, Ui, Vec2, vec2};

use crate::render::OutputPreview;

/// The tallest a preview gets when the space allows less: below this the
/// picture may run out of the visible space (the viewport still keeps its shape).
const MIN_HEIGHT: f32 = 60.0;

/// The largest rect of `canvas`'s shape (in pixels) inside `space`, centred.
/// Points or pixels don't matter: only the shape is used.
pub(crate) fn fit_picture(canvas: (u32, u32), space: Rect) -> Rect {
    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
    let s = (space.width() / cw).min(space.height() / ch).max(0.0);
    Rect::from_center_size(space.center(), vec2(cw * s, ch * s))
}

/// The size to give a preview of `canvas`: `width` across, and the picture's
/// own height at that width, but never taller than `tall` times the width
/// (a tall sub display is previewed narrower) nor than `visible` (what is left
/// of the pane in sight), unless that is under `MIN_HEIGHT`.
pub(crate) fn space_size(canvas: (u32, u32), width: f32, visible: f32, tall: f32) -> Vec2 {
    let width = width.max(1.0);
    let natural = width * canvas.1.max(1) as f32 / canvas.0.max(1) as f32;
    let h = natural
        .min(width * tall)
        .min(visible.max(MIN_HEIGHT))
        .max(1.0);
    vec2(width, h)
}

/// `space_size` at `ui`'s cursor: the width left in the ui and in sight (a row
/// wider than the pane widens the ui past its edge), the height left in sight,
/// less `margin` on every side.
pub(crate) fn space(ui: &Ui, canvas: (u32, u32), margin: f32, tall: f32) -> Vec2 {
    let clip = ui.clip_rect();
    let at = ui.cursor().min;
    let width = ui.available_width().min(clip.right() - at.x) - 2.0 * margin;
    let visible = clip.bottom() - at.y - 2.0 * margin;
    space_size(canvas, width, visible, tall)
}

/// The size of the part `area` (x, y, width, height, 0 to 1) of a `canvas`
/// pixels big: the shape of a display showing that part of another's picture.
pub(crate) fn part_shape(canvas: (u32, u32), area: [f32; 4]) -> (u32, u32) {
    (
        ((canvas.0 as f32 * area[2]).round() as u32).max(1),
        ((canvas.1 as f32 * area[3]).round() as u32).max(1),
    )
}

/// A thin line round the picture where black bars sit beside it, so the
/// display's edges show against them.
pub(crate) fn outline(ui: &Ui, picture: Rect, bars: Rect) {
    if picture.width() + 1.0 < bars.width() || picture.height() + 1.0 < bars.height() {
        ui.painter().rect_stroke(
            picture,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
            egui::StrokeKind::Outside,
        );
    }
}

/// `rect` (points) in pixels, as a viewport: x, y, width, height, not clamped
/// to the window.
pub(crate) fn viewport_px(rect: Rect, pixels_per_point: f32) -> [f32; 4] {
    [
        rect.min.x * pixels_per_point,
        rect.min.y * pixels_per_point,
        rect.width() * pixels_per_point,
        rect.height() * pixels_per_point,
    ]
}

/// An `OutputPreview` drawn through its whole rect even when part of it is out
/// of the window: egui sets the viewport clamped to the window, which would
/// squeeze the picture into the part in sight. The clip rect (already set as
/// the scissor) still keeps it to the pane.
pub(crate) struct WholeViewport(pub OutputPreview);

impl egui_wgpu::CallbackTrait for WholeViewport {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        self.0.prepare(device, queue, screen, encoder, resources)
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let clamped = info.viewport_in_pixels();
        let [x, y, w, h] = viewport_px(info.viewport, info.pixels_per_point);
        // Only when egui's was cut by the window's edge (it rounds to pixels).
        let cut =
            (clamped.width_px as f32 - w).abs() > 1.0 || (clamped.height_px as f32 - h).abs() > 1.0;
        if cut && w >= 1.0 && h >= 1.0 {
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
        }
        self.0.paint(info, pass, resources);
    }
}

/// The paint callback for a preview in `rect`.
pub(crate) fn callback(rect: Rect, preview: OutputPreview) -> egui::Shape {
    egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(
        rect,
        WholeViewport(preview),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, pos2};

    fn aspect(r: Rect) -> f32 {
        r.width() / r.height()
    }

    #[test]
    fn a_picture_keeps_its_shape_in_any_space() {
        let hd = (1920, 1080);
        // Wide space: pillarboxed, full height, centred.
        let r = fit_picture(
            hd,
            Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 200.0)),
        );
        assert!((r.height() - 200.0).abs() < 0.01, "{r:?}");
        assert!((aspect(r) - 16.0 / 9.0).abs() < 0.001);
        assert!((r.center().x - 410.0).abs() < 0.01);
        // Narrow space: letterboxed, full width.
        let r = fit_picture(hd, Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 800.0)));
        assert!((r.width() - 200.0).abs() < 0.01, "{r:?}");
        assert!((aspect(r) - 16.0 / 9.0).abs() < 0.001);
        // A square display, a tall sub display, a part of a picture.
        for canvas in [(1080, 1080), (1080, 1920), (960, 1080), (3840, 1080)] {
            for space in [vec2(300.0, 100.0), vec2(100.0, 300.0), vec2(433.0, 251.5)] {
                let s = Rect::from_min_size(pos2(5.0, 5.0), space);
                let r = fit_picture(canvas, s);
                let want = canvas.0 as f32 / canvas.1 as f32;
                assert!(
                    (aspect(r) - want).abs() < 0.001 * want,
                    "{canvas:?} {space:?}"
                );
                assert!(
                    s.expand(0.01).contains_rect(r),
                    "{canvas:?} {space:?}: {r:?}"
                );
                assert!(
                    (r.width() - s.width()).abs() < 0.01 || (r.height() - s.height()).abs() < 0.01,
                    "it touches two sides"
                );
            }
        }
        // Nothing to fit in, or a canvas with no size: no panic, nothing drawn.
        assert_eq!(
            fit_picture(hd, Rect::from_min_size(pos2(1.0, 1.0), Vec2::ZERO)).area(),
            0.0
        );
        assert!(fit_picture((0, 0), Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0))).is_finite());
    }

    #[test]
    fn the_space_is_the_pictures_height_unless_less_is_in_sight() {
        let hd = (1920, 1080);
        // Room enough: as tall as the picture at that width.
        assert_eq!(space_size(hd, 400.0, 1000.0, 0.75), vec2(400.0, 225.0));
        // A short pane: only what is in sight.
        assert_eq!(space_size(hd, 400.0, 150.0, 0.75), vec2(400.0, 150.0));
        // A tall display: capped at three quarters of the width.
        assert_eq!(
            space_size((1080, 1920), 400.0, 1000.0, 0.75),
            vec2(400.0, 300.0)
        );
        // Almost nothing in sight: a small picture still.
        assert_eq!(space_size(hd, 400.0, -30.0, 0.75).y, MIN_HEIGHT);
    }

    #[test]
    fn a_part_of_a_picture_is_laid_out_on_the_whole_picture() {
        // A display showing the left half of a 3840x1080 picture, with a
        // square still fitted (Contain) on that picture.
        let source = (3840, 1080);
        let area = [0.0, 0.0, 0.5, 1.0];
        let shape = part_shape(source, area);
        assert_eq!(shape, (1920, 1080));
        // How app.rs takes the part: the picture's (u, v) into the part's.
        let [x, y, w, h] = area;
        let part = [
            [1.0 / w, 0.0, -x / w, 0.0],
            [0.0, 1.0 / h, -y / h, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        // The square's width and height in the preview's pixels when its
        // layers are laid out on `laid_on`.
        let drawn = |laid_on: (u32, u32)| {
            let [l, t, r, b] =
                crate::gpu::fit_rect((1000, 1000), laid_on, crate::show::Fit::Contain);
            let to_uv = |nx: f32, ny: f32| ((nx + 1.0) / 2.0, (1.0 - ny) / 2.0);
            let (u0, v0) = to_uv(l, t);
            let (u1, v1) = to_uv(r, b);
            let a = crate::gpu::project(&part, u0, v0);
            let b = crate::gpu::project(&part, u1, v1);
            (
                (b[0] - a[0]).abs() * shape.0 as f32,
                (b[1] - a[1]).abs() * shape.1 as f32,
            )
        };
        // On the whole picture (as the display's own window does): square.
        let (pw, ph) = drawn(source);
        assert!((pw / ph - 1.0).abs() < 0.001, "{pw}x{ph}");
        // On the part's size (as the preview did): twice as wide.
        let (bw, bh) = drawn(shape);
        assert!((bw / bh - 2.0).abs() < 0.001, "{bw}x{bh}");
    }

    #[test]
    fn the_viewport_is_the_whole_rect_in_pixels() {
        // Off the window's right edge on a Retina screen.
        let r = Rect::from_min_size(pos2(1200.0, 100.0), vec2(160.0, 90.0));
        assert_eq!(viewport_px(r, 2.0), [2400.0, 200.0, 320.0, 180.0]);
        assert_eq!(viewport_px(r, 1.0), [1200.0, 100.0, 160.0, 90.0]);
        // egui's own would be cut at a 1280-point window's edge, and squeezed.
        let egui_px = egui::PaintCallbackInfo {
            viewport: r,
            clip_rect: r,
            pixels_per_point: 2.0,
            screen_size_px: [2560, 1600],
        }
        .viewport_in_pixels();
        assert_eq!(egui_px.width_px, 160);
        assert_eq!(egui_px.height_px, 180);
    }
}
