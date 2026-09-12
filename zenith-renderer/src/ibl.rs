use crate::helpers::upload_texture;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{BufferId, RenderGraphBuilder};
use zenith_rhi::*;

pub struct ImageBasedLightingRenderer {
    pub skybox: Arc<ImageBinding>,
    buffer: Arc<Memory>,
    pipeline: Arc<ComputePipeline>,
    sampler: Arc<Sampler>,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    output: GpuAddress,
    skybox: u32,
    sampler: u32,
}

impl ImageBasedLightingRenderer {
    pub fn new(
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        sampler: Arc<Sampler>,
    ) -> anyhow::Result<Self> {
        let mut desc = TextureDesc::color(1, 1, vk::Format::R8G8B8A8_UNORM);
        desc.cube = true;
        desc.layers = 6;
        desc.usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
        let texture = gpu.texture(desc)?;
        let buffer = gpu.allocate(144, MemoryDomain::Device)?;
        let mut commands = gpu.commands()?;
        unsafe {
            commands.initialize(&texture)?;
        }
        commands.clear_color(&texture, [0.0; 4])?;
        commands.fill(&buffer.whole(), 0)?;
        commands.barrier(Access::COPY_WRITE, Access::ALL)?;
        commands.submit()?.wait(10_000_000_000)?;
        let shader = gpu.compile_shader(
            "content/shaders/ibl_diffuse.slang",
            "main",
            ShaderStage::Compute,
        )?;
        Ok(Self {
            skybox: descriptors.image(&texture.full_view()?, false)?,
            buffer,
            pipeline: gpu.compute(&shader)?,
            sampler,
        })
    }
    pub fn set_skybox(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        asset: &zenith_asset::texture::Texture,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(asset.is_cubemap, "skybox must be a baked cubemap");
        let mut commands = gpu.commands()?;
        let texture = upload_texture(gpu, &mut commands, asset)?;
        commands.barrier(Access::COPY_WRITE, Access::ALL)?;
        commands.submit()?.wait(10_000_000_000)?;
        self.skybox = descriptors.image(&texture.full_view()?, false)?;
        Ok(())
    }
    pub fn render(&self, builder: &mut RenderGraphBuilder<'_>) -> anyhow::Result<BufferId> {
        let skybox = builder.import_sampled(self.skybox.clone())?;
        let output = builder.import_buffer(self.buffer.clone());
        let pipeline = self.pipeline.clone();
        let sampler = self.sampler.clone();
        builder.pass(
            "integrate_ibl",
            vec![
                skybox.read(Access::COMPUTE_READ),
                output.write(Access::COMPUTE_WRITE),
            ],
            move |ctx| {
                let memory = ctx.buffer(output)?;
                let data = Root {
                    output: memory.address(),
                    skybox: ctx.sampled(skybox)?,
                    sampler: ctx.sampler(&sampler)?,
                };
                let root = ctx.arguments(&data)?;
                unsafe {
                    ctx.commands
                        .dispatch(&pipeline, &root, [1, 1, 1], &[memory])
                }
            },
        )?;
        Ok(output)
    }
}
