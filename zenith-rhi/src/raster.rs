use super::pipeline_cache::{CachedPipeline, PipelineKey};
use super::shader::root_mapping;
use super::{Commands, Gpu, MemorySlice, Shader, ShaderStage, TextureView};
use anyhow::{Result, ensure};
use ash::{vk, vk::TaggedStructure};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Blend {
    pub enabled: bool,
    pub source_color: vk::BlendFactor,
    pub destination_color: vk::BlendFactor,
    pub color_op: vk::BlendOp,
    pub source_alpha: vk::BlendFactor,
    pub destination_alpha: vk::BlendFactor,
    pub alpha_op: vk::BlendOp,
    pub write_mask: vk::ColorComponentFlags,
}

impl Default for Blend {
    fn default() -> Self {
        Self {
            enabled: false,
            source_color: vk::BlendFactor::ONE,
            destination_color: vk::BlendFactor::ZERO,
            color_op: vk::BlendOp::ADD,
            source_alpha: vk::BlendFactor::ONE,
            destination_alpha: vk::BlendFactor::ZERO,
            alpha_op: vk::BlendOp::ADD,
            write_mask: vk::ColorComponentFlags::RGBA,
        }
    }
}

pub struct RasterDesc<'a> {
    pub vertex: &'a Shader,
    pub fragment: &'a Shader,
    pub colors: &'a [vk::Format],
    pub depth: vk::Format,
    pub stencil: vk::Format,
    pub samples: vk::SampleCountFlags,
    pub topology: vk::PrimitiveTopology,
    pub blend: &'a [Blend],
    pub dynamic_blend: bool,
}

pub struct RasterPipeline {
    pub(crate) gpu: Arc<Gpu>,
    pub(crate) raw: vk::Pipeline,
    signature: RenderingSignature,
}

#[derive(PartialEq, Eq)]
pub(crate) struct RenderingSignature {
    colors: Vec<vk::Format>,
    depth: vk::Format,
    stencil: vk::Format,
    samples: vk::SampleCountFlags,
}

impl Gpu {
    pub fn raster(self: &Arc<Self>, desc: &RasterDesc<'_>) -> Result<Arc<RasterPipeline>> {
        self.raster_specialized(desc, &[], &[])
    }

    pub fn raster_specialized(
        self: &Arc<Self>,
        desc: &RasterDesc<'_>,
        vertex_specialization: &[(u32, u32)],
        fragment_specialization: &[(u32, u32)],
    ) -> Result<Arc<RasterPipeline>> {
        zenith_core::profile::scope!("Raster pipeline lookup");
        let vertex_specialization = super::shader::Specialization::new(vertex_specialization)?;
        let fragment_specialization = super::shader::Specialization::new(fragment_specialization)?;
        ensure!(
            desc.vertex.heap_strides == self.descriptor_strides()?
                && desc.fragment.heap_strides == desc.vertex.heap_strides,
            "shader descriptor strides do not match the device"
        );
        ensure!(
            desc.vertex.stage == ShaderStage::Vertex
                && desc.fragment.stage == ShaderStage::Fragment,
            "raster requires vertex and fragment shaders"
        );
        ensure!(
            desc.colors.len() <= self.limits.max_color_attachments as usize
                && desc.blend.len() == desc.colors.len(),
            "invalid blend or attachment count"
        );
        ensure!(
            desc.samples.as_raw().is_power_of_two(),
            "invalid sample count"
        );
        ensure!(
            matches!(
                desc.topology,
                vk::PrimitiveTopology::POINT_LIST
                    | vk::PrimitiveTopology::LINE_LIST
                    | vk::PrimitiveTopology::LINE_STRIP
                    | vk::PrimitiveTopology::TRIANGLE_LIST
                    | vk::PrimitiveTopology::TRIANGLE_STRIP
                    | vk::PrimitiveTopology::TRIANGLE_FAN
            ),
            "topology requires an unsupported shader stage"
        );
        ensure!(
            !desc.dynamic_blend || self.dynamic_blend.is_some(),
            "dynamic blending is unsupported"
        );
        let key = PipelineKey::Raster {
            vertex: desc.vertex.code.clone(),
            fragment: desc.fragment.code.clone(),
            vertex_specialization: vertex_specialization.constants.clone(),
            fragment_specialization: fragment_specialization.constants.clone(),
            colors: desc.colors.to_vec(),
            depth: desc.depth,
            stencil: desc.stencil,
            samples: desc.samples,
            topology: desc.topology,
            blend: if desc.dynamic_blend {
                Vec::new()
            } else {
                desc.blend.to_vec()
            },
            dynamic_blend: desc.dynamic_blend,
        };
        let mut cache = self.pipelines.lock();
        if let Some(CachedPipeline::Raster(weak)) = cache.entries.get(&key) {
            if let Some(pipeline) = weak.upgrade() {
                return Ok(pipeline);
            }
        }
        cache.collect();
        let vertex_module = unsafe {
            self.raw.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&desc.vertex.code),
                None,
            )?
        };
        let fragment_module = match unsafe {
            self.raw.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&desc.fragment.code),
                None,
            )
        } {
            Ok(module) => module,
            Err(error) => {
                unsafe {
                    self.raw.destroy_shader_module(vertex_module, None);
                }
                return Err(error.into());
            }
        };
        let vertex_mappings = [root_mapping(ShaderStage::Vertex)];
        let fragment_mappings = [root_mapping(ShaderStage::Fragment)];
        let mut vm =
            vk::ShaderDescriptorSetAndBindingMappingInfoEXT::default().mappings(&vertex_mappings);
        let mut fm =
            vk::ShaderDescriptorSetAndBindingMappingInfoEXT::default().mappings(&fragment_mappings);
        let vertex_spec = vertex_specialization.info();
        let fragment_spec = fragment_specialization.info();
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .module(vertex_module)
                .name(&desc.vertex.entry)
                .stage(vk::ShaderStageFlags::VERTEX)
                .specialization_info(&vertex_spec)
                .push(&mut vm),
            vk::PipelineShaderStageCreateInfo::default()
                .module(fragment_module)
                .name(&desc.fragment.entry)
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .specialization_info(&fragment_spec)
                .push(&mut fm),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(desc.topology);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0);
        let samples =
            vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(desc.samples);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default();
        let blends: Vec<_> = desc
            .blend
            .iter()
            .map(|blend| {
                vk::PipelineColorBlendAttachmentState::default()
                    .blend_enable(blend.enabled)
                    .src_color_blend_factor(blend.source_color)
                    .dst_color_blend_factor(blend.destination_color)
                    .color_blend_op(blend.color_op)
                    .src_alpha_blend_factor(blend.source_alpha)
                    .dst_alpha_blend_factor(blend.destination_alpha)
                    .alpha_blend_op(blend.alpha_op)
                    .color_write_mask(blend.write_mask)
            })
            .collect();
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blends);
        let mut states = vec![
            vk::DynamicState::VIEWPORT,
            vk::DynamicState::SCISSOR,
            vk::DynamicState::CULL_MODE,
            vk::DynamicState::FRONT_FACE,
            vk::DynamicState::DEPTH_TEST_ENABLE,
            vk::DynamicState::DEPTH_WRITE_ENABLE,
            vk::DynamicState::DEPTH_COMPARE_OP,
            vk::DynamicState::DEPTH_BIAS_ENABLE,
            vk::DynamicState::DEPTH_BIAS,
            vk::DynamicState::STENCIL_TEST_ENABLE,
            vk::DynamicState::STENCIL_OP,
            vk::DynamicState::STENCIL_COMPARE_MASK,
            vk::DynamicState::STENCIL_WRITE_MASK,
            vk::DynamicState::STENCIL_REFERENCE,
            vk::DynamicState::BLEND_CONSTANTS,
        ];
        if desc.dynamic_blend {
            states.extend([
                vk::DynamicState::COLOR_BLEND_ENABLE_EXT,
                vk::DynamicState::COLOR_BLEND_EQUATION_EXT,
                vk::DynamicState::COLOR_WRITE_MASK_EXT,
            ]);
        }
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&states);
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(desc.colors)
            .depth_attachment_format(desc.depth)
            .stencil_attachment_format(desc.stencil);
        let mut flags = vk::PipelineCreateFlags2CreateInfo::default()
            .flags(vk::PipelineCreateFlags2::DESCRIPTOR_HEAP_EXT);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&samples)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .push(&mut rendering)
            .push(&mut flags);
        let result = unsafe {
            self.raw
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
        };
        unsafe {
            self.raw.destroy_shader_module(vertex_module, None);
            self.raw.destroy_shader_module(fragment_module, None);
        }
        match result {
            Ok(pipelines) => {
                let pipeline = Arc::new(RasterPipeline {
                    gpu: self.clone(),
                    raw: pipelines[0],
                    signature: RenderingSignature {
                        colors: desc.colors.to_vec(),
                        depth: desc.depth,
                        stencil: desc.stencil,
                        samples: desc.samples,
                    },
                });
                cache
                    .entries
                    .insert(key, CachedPipeline::Raster(Arc::downgrade(&pipeline)));
                Ok(pipeline)
            }
            Err((pipelines, error)) => {
                for pipeline in pipelines {
                    unsafe {
                        self.raw.destroy_pipeline(pipeline, None);
                    }
                }
                Err(error.into())
            }
        }
    }
}

impl Drop for RasterPipeline {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_pipeline(self.raw, None);
        }
    }
}

pub struct Attachment<'a> {
    pub view: &'a Arc<TextureView>,
    pub clear: Option<[f32; 4]>,
    pub store: bool,
    pub resolve: Option<&'a Arc<TextureView>>,
}

pub struct DepthAttachment<'a> {
    pub view: &'a Arc<TextureView>,
    pub clear: Option<(f32, u32)>,
    pub store: bool,
}

pub enum IndirectCount<'a> {
    Fixed(u32),
    Gpu { memory: &'a MemorySlice, max: u32 },
}

#[derive(Clone, Copy)]
pub struct RasterState {
    pub cull: vk::CullModeFlags,
    pub front: vk::FrontFace,
    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_compare: vk::CompareOp,
    pub depth_bias: Option<(f32, f32)>,
    pub stencil: Option<(vk::StencilOpState, vk::StencilOpState)>,
    pub blend_constants: [f32; 4],
}

impl Default for RasterState {
    fn default() -> Self {
        Self {
            cull: vk::CullModeFlags::NONE,
            front: vk::FrontFace::COUNTER_CLOCKWISE,
            depth_test: false,
            depth_write: false,
            depth_compare: vk::CompareOp::ALWAYS,
            depth_bias: None,
            stencil: None,
            blend_constants: [0.0; 4],
        }
    }
}

impl Commands {
    pub fn begin_rendering(
        &mut self,
        colors: &[Attachment<'_>],
        depth: Option<DepthAttachment<'_>>,
        extent: vk::Extent2D,
    ) -> Result<()> {
        zenith_core::profile::scope!("Begin rendering");
        ensure!(
            self.rendering.is_none() && extent.width > 0 && extent.height > 0,
            "invalid rendering scope or extent"
        );
        let validate = |view: &TextureView, usage: vk::ImageUsageFlags| -> Result<()> {
            ensure!(
                Arc::ptr_eq(&view.texture.gpu, &self.gpu)
                    && view.texture.desc.usage.contains(usage),
                "invalid attachment device or usage"
            );
            ensure!(
                view.range.level_count == 1
                    && extent.width
                        <= (view.texture.desc.extent.width >> view.range.base_mip_level).max(1)
                    && extent.height
                        <= (view.texture.desc.extent.height >> view.range.base_mip_level).max(1),
                "render extent exceeds attachment"
            );
            ensure!(
                view.kind == vk::ImageViewType::TYPE_2D
                    || view.kind == vk::ImageViewType::TYPE_2D_ARRAY,
                "invalid attachment view type"
            );
            Ok(())
        };
        let samples = colors
            .first()
            .map(|c| c.view.texture.desc.samples)
            .or_else(|| depth.as_ref().map(|d| d.view.texture.desc.samples))
            .unwrap_or(vk::SampleCountFlags::TYPE_1);
        ensure!(
            colors
                .iter()
                .all(|c| c.view.texture.desc.samples == samples)
                && depth
                    .as_ref()
                    .is_none_or(|d| d.view.texture.desc.samples == samples),
            "attachment sample counts differ"
        );
        let signature = RenderingSignature {
            colors: colors.iter().map(|c| c.view.texture.desc.format).collect(),
            depth: depth
                .as_ref()
                .filter(|d| {
                    d.view
                        .range
                        .aspect_mask
                        .contains(vk::ImageAspectFlags::DEPTH)
                })
                .map_or(vk::Format::UNDEFINED, |d| d.view.texture.desc.format),
            stencil: depth
                .as_ref()
                .filter(|d| {
                    d.view
                        .range
                        .aspect_mask
                        .contains(vk::ImageAspectFlags::STENCIL)
                })
                .map_or(vk::Format::UNDEFINED, |d| d.view.texture.desc.format),
            samples,
        };
        let mut color_infos = Vec::new();
        for color in colors {
            validate(color.view, vk::ImageUsageFlags::COLOR_ATTACHMENT)?;
            let mut info = vk::RenderingAttachmentInfo::default()
                .image_view(color.view.raw)
                .image_layout(vk::ImageLayout::GENERAL)
                .load_op(if color.clear.is_some() {
                    vk::AttachmentLoadOp::CLEAR
                } else {
                    vk::AttachmentLoadOp::LOAD
                })
                .store_op(if color.store {
                    vk::AttachmentStoreOp::STORE
                } else {
                    vk::AttachmentStoreOp::DONT_CARE
                })
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: color.clear.unwrap_or_default(),
                    },
                });
            if let Some(resolve) = color.resolve {
                validate(resolve, vk::ImageUsageFlags::COLOR_ATTACHMENT)?;
                ensure!(
                    resolve.texture.desc.samples == vk::SampleCountFlags::TYPE_1
                        && color.view.texture.desc.samples != vk::SampleCountFlags::TYPE_1
                        && resolve.texture.desc.format == color.view.texture.desc.format,
                    "invalid resolve target"
                );
                info = info
                    .resolve_image_view(resolve.raw)
                    .resolve_image_layout(vk::ImageLayout::GENERAL)
                    .resolve_mode(vk::ResolveModeFlags::AVERAGE);
            }
            color_infos.push(info);
        }
        let mut depth_info = None;
        let mut stencil_info = None;
        if let Some(depth) = &depth {
            validate(depth.view, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)?;
            let clear = depth.clear.unwrap_or((1.0, 0));
            let info = vk::RenderingAttachmentInfo::default()
                .image_view(depth.view.raw)
                .image_layout(vk::ImageLayout::GENERAL)
                .load_op(if depth.clear.is_some() {
                    vk::AttachmentLoadOp::CLEAR
                } else {
                    vk::AttachmentLoadOp::LOAD
                })
                .store_op(if depth.store {
                    vk::AttachmentStoreOp::STORE
                } else {
                    vk::AttachmentStoreOp::DONT_CARE
                })
                .clear_value(vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: clear.0,
                        stencil: clear.1,
                    },
                });
            if depth
                .view
                .range
                .aspect_mask
                .contains(vk::ImageAspectFlags::DEPTH)
            {
                depth_info = Some(info);
            }
            if depth
                .view
                .range
                .aspect_mask
                .contains(vk::ImageAspectFlags::STENCIL)
            {
                stencil_info = Some(info);
            }
        }
        let mut info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent,
            })
            .layer_count(1)
            .color_attachments(&color_infos);
        if let Some(depth) = &depth_info {
            info = info.depth_attachment(depth);
        }
        if let Some(stencil) = &stencil_info {
            info = info.stencil_attachment(stencil);
        }
        for color in colors {
            self.retained.push(color.view.clone());
            if let Some(resolve) = color.resolve {
                self.retained.push(resolve.clone());
            }
        }
        if let Some(depth) = depth {
            self.retained.push(depth.view.clone());
        }
        unsafe {
            self.gpu.raw.cmd_begin_rendering(self.raw, &info);
        }
        self.rendering = Some(signature);
        if self.gpu.dynamic_blend.is_some() {
            self.blend(&vec![Blend::default(); colors.len()])?;
        }
        self.viewport_scissor(
            vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            },
            vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent,
            },
        )?;
        self.raster_state(RasterState::default())?;
        Ok(())
    }

    pub fn viewport_scissor(&mut self, viewport: vk::Viewport, scissor: vk::Rect2D) -> Result<()> {
        let bounds = self.gpu.limits.viewport_bounds_range;
        let finite = [
            viewport.x,
            viewport.y,
            viewport.width,
            viewport.height,
            viewport.min_depth,
            viewport.max_depth,
        ]
        .into_iter()
        .all(f32::is_finite);
        ensure!(
            self.rendering.is_some()
                && finite
                && viewport.width > 0.0
                && viewport.height != 0.0
                && viewport.width <= self.gpu.limits.max_viewport_dimensions[0] as f32
                && viewport.height.abs() <= self.gpu.limits.max_viewport_dimensions[1] as f32
                && viewport.x >= bounds[0]
                && viewport.x + viewport.width <= bounds[1]
                && viewport.y.min(viewport.y + viewport.height) >= bounds[0]
                && viewport.y.max(viewport.y + viewport.height) <= bounds[1]
                && (0.0..=1.0).contains(&viewport.min_depth)
                && (0.0..=1.0).contains(&viewport.max_depth)
                && scissor.offset.x >= 0
                && scissor.offset.y >= 0
                && scissor.offset.x as u64 + scissor.extent.width as u64 <= i32::MAX as u64
                && scissor.offset.y as u64 + scissor.extent.height as u64 <= i32::MAX as u64,
            "invalid viewport/scissor"
        );
        unsafe {
            self.gpu.raw.cmd_set_viewport(self.raw, 0, &[viewport]);
            self.gpu.raw.cmd_set_scissor(self.raw, 0, &[scissor]);
        }
        Ok(())
    }

    pub fn raster_state(&mut self, state: RasterState) -> Result<()> {
        ensure!(self.rendering.is_some(), "raster state outside rendering");
        unsafe {
            let d = &self.gpu.raw;
            let c = self.raw;
            d.cmd_set_cull_mode(c, state.cull);
            d.cmd_set_front_face(c, state.front);
            d.cmd_set_depth_test_enable(c, state.depth_test);
            d.cmd_set_depth_write_enable(c, state.depth_write);
            d.cmd_set_depth_compare_op(c, state.depth_compare);
            d.cmd_set_depth_bias_enable(c, state.depth_bias.is_some());
            let (constant, slope) = state.depth_bias.unwrap_or_default();
            d.cmd_set_depth_bias(c, constant, 0.0, slope);
            d.cmd_set_stencil_test_enable(c, state.stencil.is_some());
            let (front, back) = state.stencil.unwrap_or_default();
            for (face, stencil) in [
                (vk::StencilFaceFlags::FRONT, front),
                (vk::StencilFaceFlags::BACK, back),
            ] {
                d.cmd_set_stencil_op(
                    c,
                    face,
                    stencil.fail_op,
                    stencil.pass_op,
                    stencil.depth_fail_op,
                    stencil.compare_op,
                );
                d.cmd_set_stencil_compare_mask(c, face, stencil.compare_mask);
                d.cmd_set_stencil_write_mask(c, face, stencil.write_mask);
                d.cmd_set_stencil_reference(c, face, stencil.reference);
            }
            d.cmd_set_blend_constants(c, &state.blend_constants);
        }
        Ok(())
    }

    pub fn end_rendering(&mut self) -> Result<()> {
        ensure!(self.rendering.is_some(), "rendering not active");
        unsafe {
            self.gpu.raw.cmd_end_rendering(self.raw);
        }
        self.rendering = None;
        Ok(())
    }

    pub fn blend(&mut self, blends: &[Blend]) -> Result<()> {
        ensure!(
            self.rendering
                .as_ref()
                .is_some_and(|r| r.colors.len() == blends.len()),
            "blend states do not match active rendering"
        );
        let api = self
            .gpu
            .dynamic_blend
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("dynamic blending unsupported"))?;
        if blends.is_empty() {
            return Ok(());
        }
        let enabled: Vec<vk::Bool32> = blends.iter().map(|b| b.enabled.into()).collect();
        let masks: Vec<_> = blends.iter().map(|b| b.write_mask).collect();
        let equations: Vec<_> = blends
            .iter()
            .map(|b| {
                vk::ColorBlendEquationEXT::default()
                    .src_color_blend_factor(b.source_color)
                    .dst_color_blend_factor(b.destination_color)
                    .color_blend_op(b.color_op)
                    .src_alpha_blend_factor(b.source_alpha)
                    .dst_alpha_blend_factor(b.destination_alpha)
                    .alpha_blend_op(b.alpha_op)
            })
            .collect();
        unsafe {
            api.cmd_set_color_blend_enable(self.raw, 0, &enabled);
            api.cmd_set_color_blend_equation(self.raw, 0, &equations);
            api.cmd_set_color_write_mask(self.raw, 0, &masks);
        }
        Ok(())
    }

    pub(crate) fn bind_raster(
        &mut self,
        pipeline: &Arc<RasterPipeline>,
        vertex: &MemorySlice,
        fragment: &MemorySlice,
        reachable: &[MemorySlice],
    ) -> Result<()> {
        ensure!(
            self.rendering.is_some() && Arc::ptr_eq(&pipeline.gpu, &self.gpu),
            "invalid raster device or scope"
        );
        ensure!(
            self.rendering.as_ref() == Some(&pipeline.signature),
            "pipeline and rendering attachments differ"
        );
        ensure!(
            vertex.address().value() % 8 == 0 && fragment.address().value() % 8 == 0,
            "unaligned raster roots"
        );
        self.retain(vertex)?;
        self.retain(fragment)?;
        for memory in reachable {
            self.retain(memory)?;
        }
        self.retained.push(pipeline.clone());
        self.push_root(vertex.address().value(), fragment.address().value());
        unsafe {
            self.gpu
                .raw
                .cmd_bind_pipeline(self.raw, vk::PipelineBindPoint::GRAPHICS, pipeline.raw);
        }
        Ok(())
    }

    pub unsafe fn draw(
        &mut self,
        pipeline: &Arc<RasterPipeline>,
        vertex: &MemorySlice,
        fragment: &MemorySlice,
        vertices: std::ops::Range<u32>,
        instances: std::ops::Range<u32>,
        reachable: &[MemorySlice],
    ) -> Result<()> {
        ensure!(
            vertices.start <= vertices.end && instances.start <= instances.end,
            "invalid draw range"
        );
        self.bind_raster(pipeline, vertex, fragment, reachable)?;
        unsafe {
            self.gpu.raw.cmd_draw(
                self.raw,
                vertices.end - vertices.start,
                instances.end - instances.start,
                vertices.start,
                instances.start,
            );
        }
        Ok(())
    }

    pub unsafe fn draw_indexed(
        &mut self,
        pipeline: &Arc<RasterPipeline>,
        vertex: &MemorySlice,
        fragment: &MemorySlice,
        indices: &MemorySlice,
        index_type: vk::IndexType,
        base_vertex: i32,
        instances: std::ops::Range<u32>,
        reachable: &[MemorySlice],
    ) -> Result<()> {
        let stride = match index_type {
            vk::IndexType::UINT16 => 2,
            vk::IndexType::UINT32 => 4,
            _ => anyhow::bail!("only 16/32-bit indices are supported"),
        };
        ensure!(
            indices.offset % stride == 0
                && indices.size % stride == 0
                && indices.size / stride <= u32::MAX as u64
                && instances.start <= instances.end,
            "invalid indexed draw range"
        );
        self.bind_raster(pipeline, vertex, fragment, reachable)?;
        self.retain(indices)?;
        unsafe {
            self.gpu.raw.cmd_bind_index_buffer(
                self.raw,
                indices.memory.raw,
                indices.offset,
                index_type,
            );
            self.gpu.raw.cmd_draw_indexed(
                self.raw,
                (indices.size / stride) as u32,
                instances.end - instances.start,
                0,
                base_vertex,
                instances.start,
            );
        }
        Ok(())
    }

    pub unsafe fn draw_indirect(
        &mut self,
        pipeline: &Arc<RasterPipeline>,
        vertex: &MemorySlice,
        fragment: &MemorySlice,
        indices: Option<(&MemorySlice, vk::IndexType)>,
        records: &MemorySlice,
        stride: u32,
        count: IndirectCount<'_>,
        reachable: &[MemorySlice],
    ) -> Result<()> {
        let max = match &count {
            IndirectCount::Fixed(count) => *count,
            IndirectCount::Gpu { max, .. } => *max,
        };
        let record_size = if indices.is_some() { 20u64 } else { 16u64 };
        ensure!(
            records.offset % 4 == 0
                && stride % 4 == 0
                && stride as u64 >= record_size
                && max <= self.gpu.limits.max_draw_indirect_count,
            "invalid indirect record alignment or count"
        );
        ensure!(
            max == 0 || (max as u64 - 1) * stride as u64 + record_size <= records.size,
            "indirect records exceed memory range"
        );
        if let IndirectCount::Gpu { memory, .. } = &count {
            ensure!(
                memory.offset % 4 == 0 && memory.size >= 4,
                "invalid indirect count range"
            );
            self.retain(memory)?;
        }
        if let Some((indices, kind)) = indices {
            let size = match kind {
                vk::IndexType::UINT16 => 2,
                vk::IndexType::UINT32 => 4,
                _ => anyhow::bail!("only 16/32-bit indices are supported"),
            };
            ensure!(
                indices.offset % size == 0 && indices.size % size == 0,
                "unaligned index buffer"
            );
            self.retain(indices)?;
            unsafe {
                self.gpu.raw.cmd_bind_index_buffer(
                    self.raw,
                    indices.memory.raw,
                    indices.offset,
                    kind,
                );
            }
        }
        self.retain(records)?;
        self.bind_raster(pipeline, vertex, fragment, reachable)?;
        unsafe {
            match (indices.is_some(), count) {
                (true, IndirectCount::Fixed(count)) => self.gpu.raw.cmd_draw_indexed_indirect(
                    self.raw,
                    records.memory.raw,
                    records.offset,
                    count,
                    stride,
                ),
                (false, IndirectCount::Fixed(count)) => self.gpu.raw.cmd_draw_indirect(
                    self.raw,
                    records.memory.raw,
                    records.offset,
                    count,
                    stride,
                ),
                (true, IndirectCount::Gpu { memory, max }) => {
                    self.gpu.raw.cmd_draw_indexed_indirect_count(
                        self.raw,
                        records.memory.raw,
                        records.offset,
                        memory.memory.raw,
                        memory.offset,
                        max,
                        stride,
                    )
                }
                (false, IndirectCount::Gpu { memory, max }) => {
                    self.gpu.raw.cmd_draw_indirect_count(
                        self.raw,
                        records.memory.raw,
                        records.offset,
                        memory.memory.raw,
                        memory.offset,
                        max,
                        stride,
                    )
                }
            }
        }
        Ok(())
    }
}
