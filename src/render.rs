//! Render threads. Presenting to a window can block (vsync, or for a long time
//! when the OS throttles an occluded window), so every window presents from its
//! own thread. The main thread only posts the latest frame into a mailbox and
//! never waits, so timing, input and the other outputs are unaffected.

use crate::gpu::{DrawGroup, DrawLayer, Geometry, Gpu, LayerTexture, Look, Slot};
use crate::show::{ColorAdjust, Fit, LayerTransform};
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::thread::JoinHandle;
use winit::window::Window;

/// A single-slot, latest-wins handoff between threads.
struct Mailbox<T> {
    slot: Mutex<Option<T>>,
    cv: Condvar,
}

impl<T> Mailbox<T> {
    fn new() -> Self {
        Mailbox {
            slot: Mutex::new(None),
            cv: Condvar::new(),
        }
    }

    /// Post a value. If one is already waiting, `merge(old, new)` decides what remains.
    fn put(&self, value: T, merge: impl FnOnce(T, T) -> T) {
        let mut slot = self.slot.lock();
        *slot = Some(match slot.take() {
            Some(old) => merge(old, value),
            None => value,
        });
        self.cv.notify_one();
    }

    fn take(&self) -> T {
        let mut slot = self.slot.lock();
        loop {
            if let Some(v) = slot.take() {
                return v;
            }
            self.cv.wait(&mut slot);
        }
    }
}

fn acquire(
    gpu: &Gpu,
    surface: &wgpu::Surface,
    config: &wgpu::SurfaceConfiguration,
) -> Option<wgpu::SurfaceTexture> {
    match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
            Some(f)
        }
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            surface.configure(&gpu.device, config);
            match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f)
                | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
                _ => None,
            }
        }
        _ => None,
    }
}

fn resize(
    gpu: &Gpu,
    surface: &wgpu::Surface,
    config: &mut wgpu::SurfaceConfiguration,
    size: (u32, u32),
) {
    let size = (size.0.max(1), size.1.max(1));
    if (config.width, config.height) != size {
        config.width = size.0;
        config.height = size.1;
        surface.configure(&gpu.device, config);
    }
}

// ----- Output windows ---------------------------------------------------------

#[derive(Clone)]
pub struct OutputLayer {
    pub texture: Arc<LayerTexture>,
    pub opacity: f32,
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
}

/// One output's layers, placed on a screen.
#[derive(Clone)]
pub struct OutputGroup {
    pub layers: Vec<OutputLayer>,
    pub geometry: Geometry,
}

impl OutputGroup {
    fn draw(&self) -> DrawGroup<'_> {
        DrawGroup {
            layers: self
                .layers
                .iter()
                .map(|l| DrawLayer {
                    texture: &l.texture,
                    opacity: l.opacity,
                    fit: l.fit,
                    transform: l.transform,
                    color: l.color,
                })
                .collect(),
            geometry: self.geometry.clone(),
        }
    }
}

enum OutputJob {
    Frame {
        size: (u32, u32),
        groups: Vec<OutputGroup>,
    },
    Quit,
}

pub struct OutputThread {
    mailbox: Arc<Mailbox<OutputJob>>,
    handle: JoinHandle<()>,
}

impl OutputThread {
    pub fn spawn(
        gpu: Gpu,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    ) -> OutputThread {
        let mailbox = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let handle = std::thread::Builder::new()
            .name("output-render".into())
            .spawn(move || output_loop(gpu, window, surface, config, mb))
            .expect("spawn output render thread");
        OutputThread { mailbox, handle }
    }

    /// Draw `groups` (bottom first) on the whole screen.
    pub fn submit(&self, size: (u32, u32), groups: Vec<OutputGroup>) {
        self.mailbox
            .put(OutputJob::Frame { size, groups }, |old, new| match old {
                OutputJob::Quit => OutputJob::Quit,
                _ => new,
            });
    }

    /// Ask the thread to stop. Returns its handle so the caller can keep the
    /// window alive until the surface is gone.
    pub fn quit(self) -> JoinHandle<()> {
        self.mailbox.put(OutputJob::Quit, |_, quit| quit);
        self.handle
    }
}

fn output_loop(
    gpu: Gpu,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    mailbox: Arc<Mailbox<OutputJob>>,
) {
    let pipeline = gpu.compositor.pipeline(&gpu.device, config.format);
    // NEWQ_TRACE_OUTPUT=1 logs every composited frame's layer opacities.
    let trace = std::env::var_os("NEWQ_TRACE_OUTPUT").is_some();
    loop {
        let (size, groups) = match mailbox.take() {
            OutputJob::Quit => break,
            OutputJob::Frame { size, groups } => (size, groups),
        };
        resize(&gpu, &surface, &mut config, size);
        let Some(frame) = acquire(&gpu, &surface, &config) else {
            continue;
        };
        let view = frame.texture.create_view(&Default::default());
        let draw: Vec<DrawGroup> = groups.iter().map(OutputGroup::draw).collect();
        if trace {
            let ops: Vec<String> = draw
                .iter()
                .map(|g| {
                    let o: Vec<String> = g
                        .layers
                        .iter()
                        .map(|l| format!("{:.2}", l.opacity))
                        .collect();
                    o.join(", ")
                })
                .collect();
            log::info!("output composite [{}]", ops.join(" | "));
        }
        let cmd = gpu.composite(&pipeline, &view, &draw);
        gpu.queue.submit([cmd]);
        window.pre_present_notify();
        gpu.queue.present(frame);
    }
}

// ----- Control window (egui) ----------------------------------------------------

pub struct EguiFrame {
    pub size: (u32, u32),
    pub pixels_per_point: f32,
    pub jobs: Vec<egui::ClippedPrimitive>,
    pub textures: egui::TexturesDelta,
}

enum ControlJob {
    Frame(EguiFrame),
    Quit,
}

pub struct ControlThread {
    mailbox: Arc<Mailbox<ControlJob>>,
    handle: JoinHandle<()>,
}

impl ControlThread {
    pub fn spawn(
        gpu: Gpu,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    ) -> ControlThread {
        let mailbox = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let handle = std::thread::Builder::new()
            .name("control-render".into())
            .spawn(move || control_loop(gpu, window, surface, config, mb))
            .expect("spawn control render thread");
        ControlThread { mailbox, handle }
    }

    pub fn submit(&self, frame: EguiFrame) {
        self.mailbox
            .put(ControlJob::Frame(frame), |old, new| match (old, new) {
                // A skipped frame's texture changes (e.g. font atlas) must still be applied.
                (ControlJob::Frame(mut old), ControlJob::Frame(mut new)) => {
                    let newer = std::mem::take(&mut new.textures);
                    old.textures.append(newer);
                    new.textures = std::mem::take(&mut old.textures);
                    ControlJob::Frame(new)
                }
                (ControlJob::Frame(mut old), quit) => {
                    old.textures.clear();
                    quit
                }
                (quit, ControlJob::Frame(mut new)) => {
                    new.textures.clear();
                    quit
                }
                (quit, _) => quit,
            });
    }

    pub fn quit(self) -> JoinHandle<()> {
        self.mailbox.put(ControlJob::Quit, |old, quit| {
            if let ControlJob::Frame(mut f) = old {
                f.textures.clear();
            }
            quit
        });
        self.handle
    }
}

fn control_loop(
    gpu: Gpu,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    mailbox: Arc<Mailbox<ControlJob>>,
) {
    let mut renderer = egui_wgpu::Renderer::new(
        &gpu.device,
        config.format,
        egui_wgpu::RendererOptions::default(),
    );
    renderer.callback_resources.insert(PreviewPipeline(
        gpu.compositor.pipeline(&gpu.device, config.format),
    ));
    // The textures this renderer has been given in full. A partial update to
    // one it doesn't have (egui believes an earlier renderer holds it) would
    // panic; skipping it just leaves a stale patch until egui sends it whole.
    let mut known: std::collections::HashSet<egui::TextureId> = Default::default();
    loop {
        let mut f = match mailbox.take() {
            ControlJob::Quit => break,
            ControlJob::Frame(f) => f,
        };
        for (id, deltas) in f.textures.set.drain() {
            for delta in &deltas {
                if delta.pos.is_some() && !known.contains(&id) {
                    log::warn!("skipping a partial update to a texture the renderer lacks");
                    continue;
                }
                renderer.update_texture(&gpu.device, &gpu.queue, id, delta);
                known.insert(id);
            }
        }
        let freed: Vec<egui::TextureId> = f.textures.free.drain().collect();
        resize(&gpu, &surface, &mut config, f.size);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [config.width, config.height],
            pixels_per_point: f.pixels_per_point,
        };
        if let Some(frame) = acquire(&gpu, &surface, &config) {
            let view = frame.texture.create_view(&Default::default());
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("egui"),
                });
            let mut cmds =
                renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &f.jobs, &screen);
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                renderer.render(&mut pass, &f.jobs, &screen);
            }
            cmds.push(encoder.finish());
            gpu.queue.submit(cmds);
            window.pre_present_notify();
            gpu.queue.present(frame);
        }
        for id in &freed {
            renderer.free_texture(id);
            known.remove(id);
        }
    }
}

// ----- Outputs shown inside the main window ------------------------------------

/// The compositor pipeline for the control window's format, kept in the egui
/// renderer's callback resources.
struct PreviewPipeline(wgpu::RenderPipeline);

/// Draws outputs into a rectangle of the egui UI, using the same compositor
/// as a real output window: one output on its own, or a whole screen.
pub struct OutputPreview {
    pub groups: Vec<OutputGroup>,
    /// Which uniforms to use; each kind of preview needs its own.
    pub slot: Slot,
}

impl egui_wgpu::CallbackTrait for OutputPreview {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        for g in &self.groups {
            for l in &g.layers {
                l.texture.write_uniform(
                    queue,
                    self.slot,
                    &g.geometry,
                    l.opacity,
                    &Look {
                        fit: l.fit,
                        transform: l.transform,
                        color: l.color,
                    },
                );
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(PreviewPipeline(pipeline)) = resources.get::<PreviewPipeline>() else {
            return;
        };
        pass.set_pipeline(pipeline);
        for l in self.groups.iter().flat_map(|g| &g.layers) {
            if l.opacity > 0.0 {
                l.texture.draw(self.slot, pass);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Mailbox;

    #[test]
    fn mailbox_keeps_latest_and_merges() {
        let mb = Mailbox::new();
        mb.put(1, |_, n| n);
        mb.put(2, |o, n| o + n);
        assert_eq!(mb.take(), 3);
        mb.put(5, |_, n| n);
        assert_eq!(mb.take(), 5);
    }
}
