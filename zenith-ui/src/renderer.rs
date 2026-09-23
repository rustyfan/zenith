use anyhow::{ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use egui::{epaint::Primitive, ClippedPrimitive, TextureId, TextureOptions, TexturesDelta};
use std::{collections::HashMap, ops::Range, sync::Arc};
use zenith_rendergraph::{BufferId, ImageId, RenderGraphBuilder};
use zenith_rhi::*;

const VERTEX_READ: Access = Access {
    stages: vk::PipelineStageFlags2::VERTEX_SHADER,
    access: vk::AccessFlags2::SHADER_READ,
};
const INDEX_READ: Access = Access {
    stages: vk::PipelineStageFlags2::INDEX_INPUT,
    access: vk::AccessFlags2::INDEX_READ,
};
const FRAGMENT_READ: Access = Access {
    stages: vk::PipelineStageFlags2::FRAGMENT_SHADER,
    access: vk::AccessFlags2::SHADER_READ,
};
const COLOR_BLEND: Access = Access {
    stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
    access: vk::AccessFlags2::from_raw(
        vk::AccessFlags2::COLOR_ATTACHMENT_READ.as_raw()
            | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE.as_raw(),
    ),
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    vertices: GpuAddress,
    screen_size: [f32; 2],
    image: u32,
    sampler_index: u32,
}

struct UiTexture {
    binding: Arc<ImageBinding>,
    sampler: Arc<Sampler>,
}

struct Draw {
    vertices: Range<u64>,
    indices: Range<u64>,
    image: ImageId,
    sampler: Arc<Sampler>,
    scissor: vk::Rect2D,
}

pub struct UiRenderer {
    shaders: [Shader; 4],
    pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
    composite_pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
    textures: HashMap<TextureId, UiTexture>,
    samplers: HashMap<TextureOptions, Arc<Sampler>>,
}

impl UiRenderer {
    pub fn new(gpu: &Arc<Gpu>) -> Result<Self> {
        Ok(Self {
            shaders: gpu.compile_shaders([
                ("content/shaders/egui.slang", "vsmain", ShaderStage::Vertex),
                (
                    "content/shaders/egui.slang",
                    "psmain",
                    ShaderStage::Fragment,
                ),
                (
                    "content/shaders/egui.slang",
                    "composite_vs",
                    ShaderStage::Vertex,
                ),
                (
                    "content/shaders/egui.slang",
                    "composite_ps",
                    ShaderStage::Fragment,
                ),
            ])?,
            pipeline: None,
            composite_pipeline: None,
            textures: HashMap::new(),
            samplers: HashMap::new(),
        })
    }

    pub fn paint(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        output: ImageId,
        pixels_per_point: f32,
        primitives: Vec<ClippedPrimitive>,
        mut textures: TexturesDelta,
    ) -> Result<()> {
        let updates = std::mem::take(&mut textures.set);
        let freed = std::mem::take(&mut textures.free);
        ensure!(
            pixels_per_point.is_finite() && pixels_per_point > 0.0,
            "invalid UI scale"
        );
        for (id, deltas) in updates {
            for delta in deltas {
                self.update_texture(builder, id, delta)?;
            }
        }
        let result = self.paint_overlay(builder, output, pixels_per_point, primitives);
        for id in freed {
            self.textures.remove(&id);
        }
        result
    }

    fn paint_overlay(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        output: ImageId,
        pixels_per_point: f32,
        primitives: Vec<ClippedPrimitive>,
    ) -> Result<()> {
        if primitives.is_empty() {
            return Ok(());
        }
        let desc = builder.image_desc(output)?;
        if !matches!(
            desc.format,
            vk::Format::R8G8B8A8_SRGB
                | vk::Format::B8G8R8A8_SRGB
                | vk::Format::A8B8G8R8_SRGB_PACK32
        ) {
            return self.paint_meshes(builder, output, pixels_per_point, primitives);
        }
        let overlay = builder.create_image(TextureDesc::color(
            desc.extent.width,
            desc.extent.height,
            vk::Format::R8G8B8A8_UNORM,
        ))?;
        builder.pass(
            "egui_clear",
            vec![overlay.write(Access::COPY_WRITE)],
            move |ctx| ctx.commands.clear_color(&ctx.image(overlay)?, [0.0; 4]),
        )?;
        self.paint_meshes(builder, overlay, pixels_per_point, primitives)?;
        if self
            .composite_pipeline
            .as_ref()
            .is_none_or(|(format, _)| *format != desc.format)
        {
            self.composite_pipeline = Some((
                desc.format,
                pipeline(builder.gpu(), &self.shaders[2..], desc.format)?,
            ));
        }
        let pipeline = self.composite_pipeline.as_ref().unwrap().1.clone();
        builder.pass(
            "egui_composite",
            vec![overlay.read(FRAGMENT_READ), output.read_write(COLOR_BLEND)],
            move |ctx| {
                let image = ctx.sampled(overlay)?;
                let root = ctx.arguments(&Root {
                    image,
                    ..Root::zeroed()
                })?;
                let target = ctx.view(output)?;
                ctx.commands.begin_rendering(
                    &[Attachment {
                        view: &target,
                        clear: None,
                        store: true,
                        resolve: None,
                    }],
                    None,
                    vk::Extent2D {
                        width: desc.extent.width,
                        height: desc.extent.height,
                    },
                )?;
                unsafe {
                    ctx.commands
                        .draw(&pipeline, &root, &root, 0..3, 0..1, &[])?;
                }
                ctx.commands.end_rendering()
            },
        )
    }

    fn update_texture(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        id: TextureId,
        delta: egui::epaint::ImageDelta,
    ) -> Result<()> {
        let egui::ImageData::Color(image) = delta.image;
        let [width, height] = image.size;
        ensure!(
            width > 0 && height > 0 && width.checked_mul(height) == Some(image.pixels.len()),
            "invalid UI texture data"
        );
        let sampler = if let Some(sampler) = self.samplers.get(&delta.options) {
            sampler.clone()
        } else {
            let filter = |filter| match filter {
                egui::TextureFilter::Nearest => vk::Filter::NEAREST,
                egui::TextureFilter::Linear => vk::Filter::LINEAR,
            };
            let address = match delta.options.wrap_mode {
                egui::TextureWrapMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
                egui::TextureWrapMode::Repeat => vk::SamplerAddressMode::REPEAT,
                egui::TextureWrapMode::MirroredRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
            };
            let sampler = builder.descriptors().sampler_with_filters(
                filter(delta.options.magnification),
                filter(delta.options.minification),
                address,
            )?;
            self.samplers.insert(delta.options, sampler.clone());
            sampler
        };
        let (target, offset) = if let Some([x, y]) = delta.pos {
            let texture = self
                .textures
                .get_mut(&id)
                .context("UI texture patch has no allocation")?;
            let desc = texture.binding.view().texture().desc();
            ensure!(
                x.checked_add(width)
                    .is_some_and(|end| end <= desc.extent.width as usize)
                    && y.checked_add(height)
                        .is_some_and(|end| end <= desc.extent.height as usize),
                "UI texture patch exceeds allocation"
            );
            texture.sampler = sampler;
            (
                builder.import_sampled(texture.binding.clone())?,
                vk::Offset3D {
                    x: i32::try_from(x)?,
                    y: i32::try_from(y)?,
                    z: 0,
                },
            )
        } else {
            let mut desc = TextureDesc::color(
                u32::try_from(width)?,
                u32::try_from(height)?,
                vk::Format::R8G8B8A8_UNORM,
            );
            desc.usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
            let target = builder.create_image(desc)?;
            let binding = builder
                .descriptors()
                .image(&builder.export_image(target)?.full_view()?, false)?;
            builder.import_sampled(binding.clone())?;
            self.textures.insert(id, UiTexture { binding, sampler });
            (target, vk::Offset3D::default())
        };
        let bytes = bytemuck::cast_slice(&image.pixels);
        let staging = upload(builder, bytes)?;
        let byte_count = bytes.len() as u64;
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_offset(offset)
            .image_extent(vk::Extent3D {
                width: width as u32,
                height: height as u32,
                depth: 1,
            });
        builder.pass(
            "egui_texture_upload",
            vec![
                staging.read(Access::COPY_READ),
                target.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                let source = ctx.buffer_range(staging, 0..byte_count)?;
                let destination = ctx.image(target)?;
                unsafe { ctx.commands.upload_image(&source, &destination, &[region]) }
            },
        )
    }

    fn paint_meshes(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        output: ImageId,
        pixels_per_point: f32,
        primitives: Vec<ClippedPrimitive>,
    ) -> Result<()> {
        let desc = builder.image_desc(output)?;
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut draws = Vec::new();
        for primitive in primitives {
            let Primitive::Mesh(mesh) = primitive.primitive else {
                anyhow::bail!("egui paint callbacks are unsupported");
            };
            ensure!(
                mesh.is_valid() && mesh.indices.len() % 3 == 0,
                "invalid egui mesh indices"
            );
            let Some(scissor) = clip_rect(primitive.clip_rect, pixels_per_point, extent) else {
                continue;
            };
            if mesh.indices.is_empty() {
                continue;
            }
            let texture = self
                .textures
                .get(&mesh.texture_id)
                .context("egui mesh references an unknown texture")?;
            let vertex_start =
                vertices.len() as u64 * std::mem::size_of::<egui::epaint::Vertex>() as u64;
            let index_start = indices.len() as u64 * 4;
            vertices.extend_from_slice(&mesh.vertices);
            indices.extend_from_slice(&mesh.indices);
            draws.push(Draw {
                vertices: vertex_start
                    ..vertices.len() as u64 * std::mem::size_of::<egui::epaint::Vertex>() as u64,
                indices: index_start..indices.len() as u64 * 4,
                image: builder.import_sampled(texture.binding.clone())?,
                sampler: texture.sampler.clone(),
                scissor,
            });
        }
        if draws.is_empty() {
            return Ok(());
        }
        if self
            .pipeline
            .as_ref()
            .is_none_or(|(format, _)| *format != desc.format)
        {
            self.pipeline = Some((
                desc.format,
                pipeline(builder.gpu(), &self.shaders[..2], desc.format)?,
            ));
        }
        let pipeline = self.pipeline.as_ref().unwrap().1.clone();
        let vertices = upload(builder, bytemuck::cast_slice(&vertices))?;
        let indices = upload(builder, bytemuck::cast_slice(&indices))?;
        let mut uses = vec![
            vertices.read(VERTEX_READ),
            indices.read(INDEX_READ),
            output.read_write(COLOR_BLEND),
        ];
        for draw in &draws {
            uses.push(draw.image.read(FRAGMENT_READ));
        }
        builder.pass("egui", uses, move |ctx| {
            let target = ctx.view(output)?;
            ctx.commands.begin_rendering(
                &[Attachment {
                    view: &target,
                    clear: None,
                    store: true,
                    resolve: None,
                }],
                None,
                extent,
            )?;
            for draw in draws {
                let vertices = ctx.buffer_range(vertices, draw.vertices)?;
                let indices = ctx.buffer_range(indices, draw.indices)?;
                let image = ctx.sampled(draw.image)?;
                let sampler_index = ctx.sampler(&draw.sampler)?;
                let root = ctx.arguments(&Root {
                    vertices: vertices.address(),
                    screen_size: [
                        extent.width as f32 / pixels_per_point,
                        extent.height as f32 / pixels_per_point,
                    ],
                    image,
                    sampler_index,
                })?;
                ctx.commands.viewport_scissor(
                    vk::Viewport {
                        x: 0.0,
                        y: 0.0,
                        width: extent.width as f32,
                        height: extent.height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    },
                    draw.scissor,
                )?;
                unsafe {
                    ctx.commands.draw_indexed(
                        &pipeline,
                        &root,
                        &root,
                        &indices,
                        vk::IndexType::UINT32,
                        0,
                        0..1,
                        &[vertices],
                    )?;
                }
            }
            ctx.commands.end_rendering()
        })
    }
}

fn pipeline(gpu: &Arc<Gpu>, shaders: &[Shader], format: vk::Format) -> Result<Arc<RasterPipeline>> {
    gpu.raster(&RasterDesc {
        vertex: &shaders[0],
        fragment: &shaders[1],
        colors: &[format],
        depth: vk::Format::UNDEFINED,
        stencil: vk::Format::UNDEFINED,
        samples: vk::SampleCountFlags::TYPE_1,
        topology: vk::PrimitiveTopology::TRIANGLE_LIST,
        blend: &[Blend {
            enabled: true,
            destination_color: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            destination_alpha: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            ..Default::default()
        }],
        dynamic_blend: false,
    })
}

fn upload(builder: &mut RenderGraphBuilder<'_>, bytes: &[u8]) -> Result<BufferId> {
    let buffer = builder.create_buffer(
        (bytes.len() as u64).next_power_of_two(),
        MemoryDomain::Upload,
    )?;
    builder.export_buffer(buffer)?.write(0, bytes)?;
    builder.pass(
        "egui_host_upload",
        vec![buffer.write(Access {
            stages: vk::PipelineStageFlags2::HOST,
            access: vk::AccessFlags2::HOST_WRITE,
        })],
        |_| Ok(()),
    )?;
    Ok(buffer)
}

fn clip_rect(rect: egui::Rect, scale: f32, extent: vk::Extent2D) -> Option<vk::Rect2D> {
    if rect.is_negative()
        || rect.min.x.is_nan()
        || rect.min.y.is_nan()
        || rect.max.x.is_nan()
        || rect.max.y.is_nan()
    {
        return None;
    }
    let min_x = (rect.min.x * scale).round().clamp(0.0, extent.width as f32) as u32;
    let min_y = (rect.min.y * scale)
        .round()
        .clamp(0.0, extent.height as f32) as u32;
    let max_x = (rect.max.x * scale)
        .round()
        .clamp(min_x as f32, extent.width as f32) as u32;
    let max_y = (rect.max.y * scale)
        .round()
        .clamp(min_y as f32, extent.height as f32) as u32;
    (max_x > min_x && max_y > min_y).then_some(vk::Rect2D {
        offset: vk::Offset2D {
            x: min_x as i32,
            y: min_y as i32,
        },
        extent: vk::Extent2D {
            width: max_x - min_x,
            height: max_y - min_y,
        },
    })
}

#[cfg(test)]
mod tests;
