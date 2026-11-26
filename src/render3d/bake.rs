//! The lens map's bake (mapping plan 4.1): what a mapped projector's pixels
//! see of the still screens, drawn on the graphics card from its lens and
//! packed as `mapped`'s header says, into a `LensMap`. On the stage-render
//! thread only (never on an output's path); `stage3d::lensmap::reference` is
//! the same sum on the processor, and the tests hold the two together.
//!
//! - The still scene's parts (`lensmap::still_scene`), the renderer's meshes
//!   (`Renderer::ensure_mesh`, so a venue already drawn by a view isn't sent
//!   again), drawn by a shader of its own (`bake.wgsl`, never `fs_part` or
//!   `fs_lit`): a fed screen writes where in its feed's picture each sample
//!   is, anything else (the venue, an unfed screen, a screen past the map's
//!   `MAX_FEEDS`, a front screen's back) writes nothing and is in the way.
//!   Depth decides, reversed as the views'; a fed screen is moved nearer by
//!   `lensmap::SCREEN_SLACK` of its distance, so one on a wall wins.
//! - No multisampling (packed numbers must not be averaged by the card):
//!   drawn at twice the size, then resolved (`fs_resolve`): the feed most of
//!   a pixel's four samples show, the average of their places (the pixel's
//!   middle when all four agree, which is where the reference casts its
//!   ray), and the share of them as the coverage at a screen's edge.
//! - In tiles of at most `TILE` pixels (and half the card's largest
//!   texture), each with its own part of the lens: a 4K map needs no 8K
//!   texture, and the twice-size pictures, 12 bytes a sample, are at most
//!   48 MiB, let go as soon as the map is made. The map itself is 8 bytes a
//!   pixel.
//! - All of it inside error scopes (validation, internal, out of memory): a
//!   card that refuses the pipelines refuses every bake after (said once);
//!   one bake that fails is said and returns an error. The stage view is
//!   never touched by either.
//! - Submitted through `Gpu::submit`, a tile at a time (its uniforms are
//!   written before each).

use super::{DEPTH_FORMAT, Renderer, Scene, Vertex, maths};
use crate::gpu::Gpu;
use crate::mapped::{FORMAT, LensMap};
use crate::stage3d::Projector;
use crate::stage3d::lensmap;

/// The largest tile of a map baked at once (pixels each way).
pub const TILE: u32 = 1024;

/// The bake's shader, after the map's unpacking (`mapped::WGSL_UNPACK`).
pub fn source() -> String {
    format!(
        "{}{}",
        crate::mapped::WGSL_UNPACK,
        include_str!("bake.wgsl")
    )
}

/// A shader no card takes (a test of a refusal).
#[cfg(test)]
pub(super) const BROKEN: &str = "@fragment fn fs_bake() -> @location(0) vec4<f32> { return nope; }";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BakeFrame {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BakePart {
    model: [[f32; 4]; 4],
    n: [[f32; 4]; 3],
    region: [f32; 4],
    flags: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TileUniform {
    origin: [i32; 4],
}

/// What to bake: projector `projector`'s map of `scene` (a still scene) at
/// `size`, its picture `aspect` wide per high; `key` is the map's
/// (`lensmap::map_key`).
pub struct Bake<'a> {
    pub scene: &'a Scene,
    pub projector: &'a Projector,
    pub aspect: f32,
    pub size: (u32, u32),
    pub key: u64,
}

/// The bake's pipelines and buffers, made at the first bake.
pub(super) struct BakeKit {
    part_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
    frame: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    tile: wgpu::Buffer,
    parts: wgpu::Buffer,
    parts_group: wgpu::BindGroup,
    part_capacity: u64,
    part_stride: u64,
}

impl BakeKit {
    /// None when this card won't make them.
    fn new(device: &wgpu::Device, source: &str) -> Option<BakeKit> {
        super::checked(device, "lens maps", || BakeKit::make(device, source))
    }

    fn make(device: &wgpu::Device, source: &str) -> BakeKit {
        let uniform = |binding, dynamic, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let picture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lens map frame"),
            entries: &[uniform(0, false, size_of::<BakeFrame>())],
        });
        let part_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lens map part"),
            entries: &[uniform(0, true, size_of::<BakePart>())],
        });
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lens map resolve"),
            entries: &[
                picture(1),
                picture(2),
                uniform(3, false, size_of::<TileUniform>()),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lens map"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let targets = [
            Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ];
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lens map"),
            bind_group_layouts: &[Some(&frame_layout), Some(&part_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lens map"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_bake"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                // Both faces: a front screen's back is in the way too.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_bake"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        let resolve_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("lens map resolve"),
                bind_group_layouts: &[Some(&resolve_layout)],
                immediate_size: 0,
            });
        let resolve = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lens map resolve"),
            layout: Some(&resolve_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_whole"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_resolve"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        let buffer = |label, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame = buffer("lens map frame", size_of::<BakeFrame>() as u64);
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lens map frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });
        let tile = buffer("lens map tile", size_of::<TileUniform>() as u64);
        let part_stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
            .max(size_of::<BakePart>() as u64)
            .next_multiple_of(256);
        let part_capacity = 16;
        let (parts, parts_group) =
            Self::part_buffer(device, &part_layout, part_stride, part_capacity);
        BakeKit {
            part_layout,
            resolve_layout,
            pipeline,
            resolve,
            frame,
            frame_group,
            tile,
            parts,
            parts_group,
            part_capacity,
            part_stride,
        }
    }

    fn part_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        stride: u64,
        capacity: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens map parts"),
            size: stride * capacity,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lens map parts"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<BakePart>() as u64),
                }),
            }],
        });
        (buffer, group)
    }

    fn fit_parts(&mut self, device: &wgpu::Device, wanted: u64) {
        if wanted <= self.part_capacity {
            return;
        }
        let capacity = wanted.next_power_of_two();
        let (parts, group) =
            Self::part_buffer(device, &self.part_layout, self.part_stride, capacity);
        self.parts = parts;
        self.parts_group = group;
        self.part_capacity = capacity;
    }
}

/// A picture the bake draws into at twice the size (or its depth).
fn scratch(gpu: &Gpu, size: (u32, u32), format: wgpu::TextureFormat, label: &str) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

impl Renderer {
    /// Bake `job`'s lens map (submitted through `Gpu::submit`). An error,
    /// said in the log, when this card won't (then never tried again) or
    /// this bake failed; the caller shows black.
    pub fn bake(&mut self, gpu: &Gpu, job: &Bake) -> Result<LensMap, String> {
        if self.bake_refused {
            return Err("this graphics card won't make lens maps".into());
        }
        if self.bake_kit.is_none() {
            match BakeKit::new(&gpu.device, &self.bake_source) {
                Some(k) => self.bake_kit = Some(k),
                None => {
                    log::warn!("mapped projectors can't be drawn on this graphics card");
                    self.bake_refused = true;
                    return Err("this graphics card won't make lens maps".into());
                }
            }
        }
        let device = &gpu.device;
        let size = (job.size.0.max(1), job.size.1.max(1));
        let feeds = lensmap::feeds_seen(job.scene, job.projector, job.aspect);
        // Each part's numbers.
        let mut uniforms: Vec<BakePart> = Vec::with_capacity(job.scene.parts.len());
        let mut draws: Vec<u64> = Vec::with_capacity(job.scene.parts.len());
        for part in &job.scene.parts {
            if !part.transform.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            if !self.ensure_mesh(device, part) {
                continue;
            }
            let feed = lensmap::part_feed(part, &feeds).map_or(0.0, |f| f32::from(f) + 1.0);
            let n = maths::normal_matrix(&part.transform);
            uniforms.push(BakePart {
                model: part.transform,
                n: n.map(|c| [c[0], c[1], c[2], 0.0]),
                region: lensmap::part_region(part),
                flags: [
                    feed,
                    if feed > 0.0 && part.catch.front_only {
                        1.0
                    } else {
                        0.0
                    },
                    0.0,
                    0.0,
                ],
            });
            draws.push(part.mesh_key);
        }
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let made = self.bake_tiles(gpu, job, size, &uniforms, &draws);
        let errors = [
            pollster::block_on(memory.pop()),
            pollster::block_on(internal.pop()),
            pollster::block_on(validation.pop()),
        ];
        if let Some(e) = errors.into_iter().flatten().next() {
            log::warn!("a mapped projector's lens map couldn't be made: {e}");
            return Err(format!("the lens map couldn't be made: {e}"));
        }
        let now = std::time::Instant::now();
        for key in &draws {
            if let Some(m) = self.meshes.get_mut(key) {
                m.used = now;
            }
        }
        let (place, how) = made;
        Ok(LensMap::new(size, place, how, None, feeds, job.key))
    }

    /// The map's two pictures, drawn tile by tile.
    fn bake_tiles(
        &mut self,
        gpu: &Gpu,
        job: &Bake,
        size: (u32, u32),
        uniforms: &[BakePart],
        draws: &[u64],
    ) -> (wgpu::Texture, wgpu::Texture) {
        let device = &gpu.device;
        let kit = self.bake_kit.as_mut().expect("made above");
        kit.fit_parts(device, uniforms.len() as u64);
        if !uniforms.is_empty() {
            let stride = kit.part_stride as usize;
            let mut bytes = vec![0u8; uniforms.len() * stride];
            for (i, u) in uniforms.iter().enumerate() {
                bytes[i * stride..i * stride + size_of::<BakePart>()]
                    .copy_from_slice(bytemuck::bytes_of(u));
            }
            gpu.queue.write_buffer(&kit.parts, 0, &bytes);
        }
        let kit = self.bake_kit.as_ref().expect("made above");
        let place = LensMap::texture(gpu, size, "lens map place");
        let how = LensMap::texture(gpu, size, "lens map how");
        let place_view = place.create_view(&Default::default());
        let how_view = how.create_view(&Default::default());
        let most = (device.limits().max_texture_dimension_2d / 2).max(1);
        let tile = self.bake_tile.clamp(1, most);
        let big = (2 * tile.min(size.0), 2 * tile.min(size.1));
        let big_place = scratch(gpu, big, FORMAT, "lens map place, twice");
        let big_how = scratch(gpu, big, FORMAT, "lens map how, twice");
        let depth = scratch(gpu, big, DEPTH_FORMAT, "lens map depth");
        let big_place_view = big_place.create_view(&Default::default());
        let big_how_view = big_how.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let resolve_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lens map resolve"),
            layout: &kit.resolve_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&big_place_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&big_how_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: kit.tile.as_entire_binding(),
                },
            ],
        });
        // A fed screen's clip z moved by this is its distance taken
        // `SCREEN_SLACK` shorter (the projection's z = a - b·d; `a` its
        // [3][2]).
        let push = {
            use crate::stage3d::light::{THROW_FAR, THROW_NEAR};
            THROW_NEAR * THROW_FAR / (THROW_FAR - THROW_NEAR) * lensmap::SCREEN_SLACK
        };
        let eye = job.projector.position();
        for ty in (0..size.1).step_by(tile as usize) {
            for tx in (0..size.0).step_by(tile as usize) {
                let (tw, th) = (tile.min(size.0 - tx), tile.min(size.1 - ty));
                let frame = BakeFrame {
                    view_proj: lensmap::tile_matrix(
                        job.projector,
                        job.aspect,
                        size,
                        [tx, ty, tw, th],
                    ),
                    eye: [eye[0], eye[1], eye[2], 1.0],
                    extra: [push, 0.0, 0.0, 0.0],
                };
                gpu.queue
                    .write_buffer(&kit.frame, 0, bytemuck::bytes_of(&frame));
                let t = TileUniform {
                    origin: [tx as i32, ty as i32, 0, 0],
                };
                gpu.queue.write_buffer(&kit.tile, 0, bytemuck::bytes_of(&t));
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("lens map"),
                });
                {
                    let clear = |view| {
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })
                    };
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("lens map, twice"),
                        color_attachments: &[clear(&big_place_view), clear(&big_how_view)],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.0),
                                store: wgpu::StoreOp::Discard,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_viewport(0.0, 0.0, (2 * tw) as f32, (2 * th) as f32, 0.0, 1.0);
                    pass.set_pipeline(&kit.pipeline);
                    pass.set_bind_group(0, &kit.frame_group, &[]);
                    for (i, key) in draws.iter().enumerate() {
                        let Some(mesh) = self.meshes.get(key) else {
                            continue;
                        };
                        let offset = (i as u64 * kit.part_stride) as u32;
                        pass.set_bind_group(1, &kit.parts_group, &[offset]);
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                }
                {
                    let keep = |view| {
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })
                    };
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("lens map resolve"),
                        color_attachments: &[keep(&place_view), keep(&how_view)],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_viewport(tx as f32, ty as f32, tw as f32, th as f32, 0.0, 1.0);
                    pass.set_pipeline(&kit.resolve);
                    pass.set_bind_group(0, &resolve_group, &[]);
                    pass.draw(0..3, 0..1);
                }
                // Before the next tile's numbers are written.
                gpu.submit([encoder.finish()]);
            }
        }
        (place, how)
    }

    /// Bake maps in tiles of at most `side` pixels (for a test of the tiles).
    #[cfg(test)]
    pub fn bake_in_tiles(&mut self, side: u32) {
        self.bake_tile = side.max(1);
    }

    /// Make the bake with a shader that won't compile, as a card that
    /// refuses it would (for a test).
    #[cfg(test)]
    pub fn refuse_bake(&mut self) {
        self.bake_kit = None;
        self.bake_source = BROKEN.into();
    }

    /// Whether this card has refused the bake.
    pub fn bake_refused(&self) -> bool {
        self.bake_refused
    }
}

#[cfg(test)]
#[path = "bake_tests.rs"]
mod tests;
