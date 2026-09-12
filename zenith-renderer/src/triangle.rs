use crate::helpers::*;
use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use std::{sync::Arc, time::Instant};
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    vertices: GpuAddress,
    time: f32,
    padding: u32,
}

pub struct TriangleRenderer {
    vertices: Arc<Memory>,
    indices: Arc<Memory>,
    vertex: Shader,
    fragment: Shader,
    pipeline: Option<(vk::Format, Arc<RasterPipeline>)>,
    start: Instant,
}

impl TriangleRenderer {
    pub fn new(gpu: &Arc<Gpu>) -> Result<Self> {
        let vertices = [
            Vertex {
                position: [0.0, 0.5, 0.0],
                color: [1.0, 0.0, 0.0],
            },
            Vertex {
                position: [-0.5, -0.5, 0.0],
                color: [0.0, 1.0, 0.0],
            },
            Vertex {
                position: [0.5, -0.5, 0.0],
                color: [0.0, 0.0, 1.0],
            },
        ];
        let mut commands = gpu.commands()?;
        let vertices = upload_buffer(gpu, &mut commands, bytemuck::cast_slice(&vertices))?;
        let indices = upload_buffer(gpu, &mut commands, bytemuck::cast_slice(&[0u16, 1, 2]))?;
        commands.barrier(Access::COPY_WRITE, Access::ALL)?;
        commands.submit()?.wait(10_000_000_000)?;
        Ok(Self {
            vertices,
            indices,
            vertex: gpu.compile_shader(
                "content/shaders/triangle.slang",
                "vsmain",
                ShaderStage::Vertex,
            )?,
            fragment: gpu.compile_shader(
                "content/shaders/triangle.slang",
                "psmain",
                ShaderStage::Fragment,
            )?,
            pipeline: None,
            start: Instant::now(),
        })
    }
    pub fn render(&mut self, builder: &mut RenderGraphBuilder<'_>, output: ImageId) -> Result<()> {
        let desc = builder.image_desc(output)?;
        let extent = vk::Extent2D {
            width: desc.extent.width,
            height: desc.extent.height,
        };
        if self
            .pipeline
            .as_ref()
            .is_none_or(|(format, _)| *format != desc.format)
        {
            self.pipeline = Some((
                desc.format,
                raster(
                    builder.gpu(),
                    &self.vertex,
                    &self.fragment,
                    &[desc.format],
                    vk::Format::UNDEFINED,
                )?,
            ));
        }
        let pipeline = self.pipeline.as_ref().unwrap().1.clone();
        let vertices = builder.import_buffer(self.vertices.clone());
        let indices = builder.import_buffer(self.indices.clone());
        let time = std::env::var("ZENITH_TEST_TIME")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| self.start.elapsed().as_secs_f32());
        builder.pass(
            "triangle",
            vec![
                vertices.read(VERTEX_READ),
                indices.read(INDEX_READ),
                output.write(COLOR_WRITE),
            ],
            move |ctx| {
                let vertices = ctx.buffer(vertices)?;
                let indices = ctx.buffer(indices)?;
                let root = ctx.arguments(&Root {
                    vertices: vertices.address(),
                    time,
                    padding: 0,
                })?;
                let view = ctx.view(output)?;
                ctx.commands.begin_rendering(
                    &[Attachment {
                        view: &view,
                        clear: Some([0.1, 0.1, 0.1, 1.0]),
                        store: true,
                        resolve: None,
                    }],
                    None,
                    extent,
                )?;
                viewport(ctx.commands, extent)?;
                unsafe {
                    ctx.commands.draw_indexed(
                        &pipeline,
                        &root,
                        &root,
                        &indices,
                        vk::IndexType::UINT16,
                        0,
                        0..1,
                        &[vertices],
                    )?;
                }
                ctx.commands.end_rendering()
            },
        )
    }
}
