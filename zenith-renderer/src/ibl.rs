use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_rendergraph::{BufferId, ImageId, RenderGraphBuilder, ResourceCache};
use zenith_rhi::*;

const BRDF_LUT_SIZE: u32 = 128;
const SPECULAR_SIZE: u32 = 128;
const SH_BYTES: u64 = 7 * 16;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub(crate) struct IblResources {
    pub skybox: ImageId,
    pub sh: BufferId,
    pub specular: ImageId,
    pub brdf_lut: ImageId,
    pub max_mip: f32,
}

pub struct ImageBasedLightingRenderer {
    gpu: Arc<Gpu>,
    descriptors: Arc<Descriptors>,
    skybox: Arc<ImageBinding>,
    buffer: Arc<Memory>,
    specular: Arc<ImageBinding>,
    brdf_lut: Arc<ImageBinding>,
    diffuse_pipeline: Arc<ComputePipeline>,
    specular_pipeline: Arc<ComputePipeline>,
    sampler: Arc<Sampler>,
    submissions: Vec<Submission>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DiffuseRoot {
    output: GpuAddress,
    skybox: u32,
    sampler: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpecularRoot {
    skybox: u32,
    sampler: u32,
    output: u32,
    size: u32,
    roughness: f32,
    source_size: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LutRoot {
    output: u32,
    size: u32,
}

impl ImageBasedLightingRenderer {
    pub fn new(
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        sampler: Arc<Sampler>,
        [diffuse, specular, lut]: [&Shader; 3],
    ) -> anyhow::Result<Self> {
        let diffuse_pipeline = gpu.compute(diffuse)?;
        let specular_pipeline = gpu.compute(specular)?;
        let lut_pipeline = gpu.compute(lut)?;
        let mut cache = ResourceCache::default();
        let mut builder = RenderGraphBuilder::new(gpu, descriptors, &mut cache)?;
        let mut desc = TextureDesc::color(1, 1, vk::Format::R8G8B8A8_UNORM);
        desc.cube = true;
        desc.layers = 6;
        desc.usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
        let black = builder.create_image(desc)?;
        let sh = builder.create_buffer(SH_BYTES, MemoryDomain::Device)?;
        builder.pass(
            "initialize_ibl",
            vec![
                black.write(Access::COPY_WRITE),
                sh.write(Access::COPY_WRITE),
            ],
            move |ctx| {
                ctx.commands.clear_color(&ctx.image(black)?, [0.0; 4])?;
                ctx.commands.fill(&ctx.buffer(sh)?, 0)
            },
        )?;
        let mut desc = TextureDesc::color(BRDF_LUT_SIZE, BRDF_LUT_SIZE, vk::Format::R32G32_SFLOAT);
        desc.usage = vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC;
        let lut = builder.create_image(desc)?;
        builder.pass(
            "integrate_brdf",
            vec![lut.write(Access::COMPUTE_WRITE)],
            move |ctx| {
                let output = ctx.storage(lut)?;
                let root = ctx.arguments(&LutRoot {
                    output,
                    size: BRDF_LUT_SIZE,
                })?;
                unsafe {
                    ctx.commands.dispatch(
                        &lut_pipeline,
                        &root,
                        [BRDF_LUT_SIZE / 8, BRDF_LUT_SIZE / 8, 1],
                        &[],
                    )
                }
            },
        )?;
        let skybox = descriptors.image(&builder.export_image(black)?.full_view()?, false)?;
        let buffer = builder.export_buffer(sh)?;
        let brdf_lut = descriptors.image(&builder.export_image(lut)?.full_view()?, false)?;
        let submission = builder.record()?.submit()?;
        Ok(Self {
            gpu: gpu.clone(),
            descriptors: descriptors.clone(),
            specular: skybox.clone(),
            skybox,
            buffer,
            brdf_lut,
            diffuse_pipeline,
            specular_pipeline,
            sampler,
            submissions: vec![submission],
        })
    }

    pub fn set_skybox(&mut self, skybox: Arc<ImageBinding>) -> anyhow::Result<()> {
        if Arc::ptr_eq(&self.skybox, &skybox) {
            return Ok(());
        }
        let mut cache = ResourceCache::default();
        let mut builder = RenderGraphBuilder::new(&self.gpu, &self.descriptors, &mut cache)?;
        let source = builder.import_sampled(skybox.clone())?;
        let sh = builder.create_buffer(SH_BYTES, MemoryDomain::Device)?;
        let pipeline = self.diffuse_pipeline.clone();
        let sampler = self.sampler.clone();
        builder.pass(
            "integrate_ibl_diffuse",
            vec![
                source.read(Access::COMPUTE_READ),
                sh.write(Access::COMPUTE_WRITE),
            ],
            move |ctx| {
                let output = ctx.buffer(sh)?;
                let skybox = ctx.sampled(source)?;
                let sampler = ctx.sampler(&sampler)?;
                let root = ctx.arguments(&DiffuseRoot {
                    output: output.address(),
                    skybox,
                    sampler,
                })?;
                unsafe {
                    ctx.commands
                        .dispatch(&pipeline, &root, [1, 1, 1], &[output])
                }
            },
        )?;
        let mut desc = TextureDesc::color(
            SPECULAR_SIZE,
            SPECULAR_SIZE,
            vk::Format::R32G32B32A32_SFLOAT,
        );
        desc.cube = true;
        desc.layers = 6;
        desc.mip_levels = SPECULAR_SIZE.ilog2() + 1;
        desc.usage = vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::TRANSFER_SRC;
        let filtered = builder.create_image(desc)?;
        let source_size = skybox.view().texture().desc().extent.width as f32;
        for mip in 0..desc.mip_levels {
            let size = (SPECULAR_SIZE >> mip).max(1);
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(mip)
                .level_count(1)
                .layer_count(6);
            let pipeline = self.specular_pipeline.clone();
            let sampler = self.sampler.clone();
            builder.pass(
                format!("prefilter_ibl_{mip}"),
                vec![
                    source.read(Access::COMPUTE_READ),
                    filtered.write(Access::COMPUTE_WRITE).subresources(range),
                ],
                move |ctx| {
                    let view = ctx.view_range(filtered, vk::ImageViewType::TYPE_2D_ARRAY, range)?;
                    let output = ctx.descriptors.image(&view, true)?;
                    ctx.commands.retain_image(&output)?;
                    let skybox = ctx.sampled(source)?;
                    let sampler = ctx.sampler(&sampler)?;
                    let root = ctx.arguments(&SpecularRoot {
                        skybox,
                        sampler,
                        output: output.index(),
                        size,
                        roughness: mip as f32 / (desc.mip_levels - 1) as f32,
                        source_size,
                    })?;
                    unsafe {
                        ctx.commands.dispatch(
                            &pipeline,
                            &root,
                            [size.div_ceil(8), size.div_ceil(8), 6],
                            &[],
                        )
                    }
                },
            )?;
        }
        let buffer = builder.export_buffer(sh)?;
        let specular = self
            .descriptors
            .image(&builder.export_image(filtered)?.full_view()?, false)?;
        // Both graphs submit to queue 0; imported-resource barriers cover these writes before lighting.
        let submission = builder.record()?.submit()?;
        self.submissions.push(submission);
        self.skybox = skybox;
        self.buffer = buffer;
        self.specular = specular;
        Ok(())
    }

    pub fn render(&mut self, builder: &mut RenderGraphBuilder<'_>) -> anyhow::Result<IblResources> {
        let mut i = 0;
        while i < self.submissions.len() {
            if self.submissions[i].poll()? {
                self.submissions.swap_remove(i);
            } else {
                i += 1;
            }
        }
        Ok(IblResources {
            skybox: builder.import_sampled(self.skybox.clone())?,
            sh: builder.import_buffer(self.buffer.clone()),
            specular: builder.import_sampled(self.specular.clone())?,
            brdf_lut: builder.import_sampled(self.brdf_lut.clone())?,
            max_mip: (self.specular.view().texture().desc().mip_levels - 1) as f32,
        })
    }
}
