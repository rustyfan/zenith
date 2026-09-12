use super::{Access, Commands, Gpu, MemorySlice};
use anyhow::{Result, ensure};
use ash::vk;
use std::sync::Arc;
use vk_mem::Alloc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureDesc {
    pub extent: vk::Extent3D,
    pub format: vk::Format,
    pub usage: vk::ImageUsageFlags,
    pub mip_levels: u32,
    pub layers: u32,
    pub kind: vk::ImageType,
    pub cube: bool,
    pub samples: vk::SampleCountFlags,
}

impl TextureDesc {
    pub fn color(width: u32, height: u32, format: vk::Format) -> Self {
        Self {
            extent: vk::Extent3D {
                width,
                height,
                depth: 1,
            },
            format,
            usage: vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
            mip_levels: 1,
            layers: 1,
            kind: vk::ImageType::TYPE_2D,
            cube: false,
            samples: vk::SampleCountFlags::TYPE_1,
        }
    }
    pub fn aspect(&self) -> vk::ImageAspectFlags {
        match self.format {
            vk::Format::D16_UNORM | vk::Format::D32_SFLOAT | vk::Format::X8_D24_UNORM_PACK32 => {
                vk::ImageAspectFlags::DEPTH
            }
            vk::Format::D24_UNORM_S8_UINT
            | vk::Format::D32_SFLOAT_S8_UINT
            | vk::Format::D16_UNORM_S8_UINT => {
                vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
            }
            vk::Format::S8_UINT => vk::ImageAspectFlags::STENCIL,
            _ => vk::ImageAspectFlags::COLOR,
        }
    }
    pub fn range(&self) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange::default()
            .aspect_mask(self.aspect())
            .base_mip_level(0)
            .level_count(self.mip_levels)
            .base_array_layer(0)
            .layer_count(self.layers)
    }
}

pub struct Texture {
    pub(crate) gpu: Arc<Gpu>,
    pub(crate) raw: vk::Image,
    pub(crate) allocation: Option<vk_mem::Allocation>,
    pub(crate) desc: TextureDesc,
    pub(crate) _owner: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl Gpu {
    pub fn texture(self: &Arc<Self>, desc: TextureDesc) -> Result<Arc<Texture>> {
        let e = desc.extent;
        ensure!(
            e.width > 0 && e.height > 0 && e.depth > 0 && desc.mip_levels > 0 && desc.layers > 0,
            "empty texture"
        );
        ensure!(
            desc.mip_levels <= 32 - e.width.max(e.height).max(e.depth).leading_zeros(),
            "too many mip levels"
        );
        ensure!(
            !desc.cube
                || (desc.kind == vk::ImageType::TYPE_2D
                    && e.width == e.height
                    && desc.layers % 6 == 0),
            "invalid cube texture"
        );
        ensure!(
            desc.kind != vk::ImageType::TYPE_3D || desc.layers == 1,
            "3D texture cannot have array layers"
        );
        ensure!(
            desc.kind != vk::ImageType::TYPE_1D || (e.height == 1 && e.depth == 1),
            "invalid 1D extent"
        );
        ensure!(
            desc.kind != vk::ImageType::TYPE_2D || e.depth == 1,
            "invalid 2D extent"
        );
        let flags = if desc.cube {
            vk::ImageCreateFlags::CUBE_COMPATIBLE
        } else {
            vk::ImageCreateFlags::empty()
        };
        let supported = unsafe {
            self.instance
                .raw
                .get_physical_device_image_format_properties(
                    self.physical,
                    desc.format,
                    desc.kind,
                    vk::ImageTiling::OPTIMAL,
                    desc.usage,
                    flags,
                )?
        };
        ensure!(
            e.width <= supported.max_extent.width
                && e.height <= supported.max_extent.height
                && e.depth <= supported.max_extent.depth
                && desc.layers <= supported.max_array_layers
                && desc.mip_levels <= supported.max_mip_levels
                && supported.sample_counts.contains(desc.samples)
                && desc.samples.as_raw().is_power_of_two(),
            "texture exceeds device format limits"
        );
        let create = vk::ImageCreateInfo::default()
            .flags(flags)
            .image_type(desc.kind)
            .format(desc.format)
            .extent(e)
            .mip_levels(desc.mip_levels)
            .array_layers(desc.layers)
            .samples(desc.samples)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(desc.usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let (raw, allocation) = unsafe {
            self.allocator().create_image(
                &create,
                &vk_mem::AllocationCreateInfo {
                    usage: vk_mem::MemoryUsage::AutoPreferDevice,
                    ..Default::default()
                },
            )?
        };
        Ok(Arc::new(Texture {
            gpu: self.clone(),
            raw,
            allocation: Some(allocation),
            desc,
            _owner: None,
        }))
    }
}

impl Texture {
    pub fn desc(&self) -> TextureDesc {
        self.desc
    }
    pub fn view(
        self: &Arc<Self>,
        kind: vk::ImageViewType,
        range: vk::ImageSubresourceRange,
    ) -> Result<Arc<TextureView>> {
        ensure!(
            range.level_count > 0
                && range.layer_count > 0
                && range
                    .base_mip_level
                    .checked_add(range.level_count)
                    .is_some_and(|v| v <= self.desc.mip_levels)
                && range
                    .base_array_layer
                    .checked_add(range.layer_count)
                    .is_some_and(|v| v <= self.desc.layers),
            "texture view out of bounds"
        );
        ensure!(
            !range.aspect_mask.is_empty() && self.desc.aspect().contains(range.aspect_mask),
            "invalid view aspect"
        );
        let valid_kind = match kind {
            vk::ImageViewType::TYPE_1D => {
                self.desc.kind == vk::ImageType::TYPE_1D && range.layer_count == 1
            }
            vk::ImageViewType::TYPE_1D_ARRAY => self.desc.kind == vk::ImageType::TYPE_1D,
            vk::ImageViewType::TYPE_2D => {
                self.desc.kind == vk::ImageType::TYPE_2D && range.layer_count == 1
            }
            vk::ImageViewType::TYPE_2D_ARRAY => self.desc.kind == vk::ImageType::TYPE_2D,
            vk::ImageViewType::TYPE_3D => self.desc.kind == vk::ImageType::TYPE_3D,
            vk::ImageViewType::CUBE => self.desc.cube && range.layer_count == 6,
            vk::ImageViewType::CUBE_ARRAY => self.desc.cube && range.layer_count % 6 == 0,
            _ => false,
        };
        ensure!(valid_kind, "view type incompatible with image");
        let info = vk::ImageViewCreateInfo::default()
            .image(self.raw)
            .view_type(kind)
            .format(self.desc.format)
            .subresource_range(range);
        let raw = unsafe { self.gpu.raw.create_image_view(&info, None)? };
        Ok(Arc::new(TextureView {
            texture: self.clone(),
            raw,
            kind,
            range,
        }))
    }
    pub fn full_view(self: &Arc<Self>) -> Result<Arc<TextureView>> {
        let kind = match (self.desc.kind, self.desc.layers, self.desc.cube) {
            (_, 6, true) => vk::ImageViewType::CUBE,
            (_, _, true) => vk::ImageViewType::CUBE_ARRAY,
            (vk::ImageType::TYPE_1D, 1, _) => vk::ImageViewType::TYPE_1D,
            (vk::ImageType::TYPE_1D, _, _) => vk::ImageViewType::TYPE_1D_ARRAY,
            (vk::ImageType::TYPE_2D, 1, _) => vk::ImageViewType::TYPE_2D,
            (vk::ImageType::TYPE_2D, _, _) => vk::ImageViewType::TYPE_2D_ARRAY,
            _ => vk::ImageViewType::TYPE_3D,
        };
        self.view(kind, self.desc.range())
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if let Some(allocation) = &mut self.allocation {
            unsafe {
                self.gpu.allocator().destroy_image(self.raw, allocation);
            }
        }
    }
}

pub struct TextureView {
    pub(crate) texture: Arc<Texture>,
    pub(crate) raw: vk::ImageView,
    pub(crate) kind: vk::ImageViewType,
    pub(crate) range: vk::ImageSubresourceRange,
}
impl TextureView {
    pub fn texture(&self) -> &Arc<Texture> {
        &self.texture
    }
    pub fn range(&self) -> vk::ImageSubresourceRange {
        self.range
    }
}
impl Drop for TextureView {
    fn drop(&mut self) {
        unsafe {
            self.texture.gpu.raw.destroy_image_view(self.raw, None);
        }
    }
}

fn block_size(format: vk::Format, aspect: vk::ImageAspectFlags) -> Result<(u64, u64, u64)> {
    if aspect == vk::ImageAspectFlags::STENCIL {
        return Ok((1, 1, 1));
    }
    let bytes = match format {
        vk::Format::R8_UNORM | vk::Format::R8_UINT => 1,
        vk::Format::R8G8_UNORM | vk::Format::R16_SFLOAT | vk::Format::D16_UNORM => 2,
        vk::Format::R8G8B8A8_UNORM
        | vk::Format::R8G8B8A8_SRGB
        | vk::Format::B8G8R8A8_UNORM
        | vk::Format::B8G8R8A8_SRGB
        | vk::Format::R16G16_SFLOAT
        | vk::Format::R32_SFLOAT
        | vk::Format::R32_UINT
        | vk::Format::D32_SFLOAT
        | vk::Format::D24_UNORM_S8_UINT
        | vk::Format::D32_SFLOAT_S8_UINT => 4,
        vk::Format::R16G16B16A16_SFLOAT | vk::Format::R32G32_SFLOAT => 8,
        vk::Format::R32G32B32A32_SFLOAT => 16,
        vk::Format::BC1_RGBA_UNORM_BLOCK
        | vk::Format::BC1_RGBA_SRGB_BLOCK
        | vk::Format::BC1_RGB_UNORM_BLOCK
        | vk::Format::BC1_RGB_SRGB_BLOCK
        | vk::Format::BC4_UNORM_BLOCK
        | vk::Format::BC4_SNORM_BLOCK => return Ok((4, 4, 8)),
        vk::Format::BC2_UNORM_BLOCK
        | vk::Format::BC2_SRGB_BLOCK
        | vk::Format::BC3_UNORM_BLOCK
        | vk::Format::BC3_SRGB_BLOCK
        | vk::Format::BC5_UNORM_BLOCK
        | vk::Format::BC5_SNORM_BLOCK
        | vk::Format::BC6H_UFLOAT_BLOCK
        | vk::Format::BC6H_SFLOAT_BLOCK
        | vk::Format::BC7_UNORM_BLOCK
        | vk::Format::BC7_SRGB_BLOCK => return Ok((4, 4, 16)),
        _ => anyhow::bail!("buffer/image copy format is not supported: {format:?}"),
    };
    Ok((1, 1, bytes))
}

fn checked_regions(
    texture: &Texture,
    memory: &MemorySlice,
    regions: &[vk::BufferImageCopy],
) -> Result<Vec<vk::BufferImageCopy>> {
    ensure!(
        !regions.is_empty() && texture.desc.samples == vk::SampleCountFlags::TYPE_1,
        "empty image copy or multisampled image"
    );
    let mut adjusted = Vec::with_capacity(regions.len());
    for r in regions {
        let s = r.image_subresource;
        let e = r.image_extent;
        let o = r.image_offset;
        ensure!(
            s.mip_level < texture.desc.mip_levels
                && s.layer_count > 0
                && s.base_array_layer
                    .checked_add(s.layer_count)
                    .is_some_and(|v| v <= texture.desc.layers)
                && s.aspect_mask.as_raw().is_power_of_two()
                && texture.desc.aspect().contains(s.aspect_mask),
            "invalid image-copy subresource"
        );
        ensure!(
            e.width > 0 && e.height > 0 && e.depth > 0 && o.x >= 0 && o.y >= 0 && o.z >= 0,
            "invalid image-copy extent"
        );
        let mip = vk::Extent3D {
            width: (texture.desc.extent.width >> s.mip_level).max(1),
            height: (texture.desc.extent.height >> s.mip_level).max(1),
            depth: (texture.desc.extent.depth >> s.mip_level).max(1),
        };
        ensure!(
            o.x as u64 + e.width as u64 <= mip.width as u64
                && o.y as u64 + e.height as u64 <= mip.height as u64
                && o.z as u64 + e.depth as u64 <= mip.depth as u64,
            "image copy out of bounds"
        );
        let (bw, bh, bytes) = block_size(texture.desc.format, s.aspect_mask)?;
        let offset = memory
            .offset
            .checked_add(r.buffer_offset)
            .ok_or_else(|| anyhow::anyhow!("copy offset overflow"))?;
        ensure!(
            offset % bytes.max(4) == 0 && o.x as u64 % bw == 0 && o.y as u64 % bh == 0,
            "image copy is not block aligned"
        );
        ensure!(
            (e.width as u64 % bw == 0 || o.x as u64 + e.width as u64 == mip.width as u64)
                && (e.height as u64 % bh == 0 || o.y as u64 + e.height as u64 == mip.height as u64),
            "compressed copy does not cover complete blocks"
        );
        ensure!(
            (r.buffer_row_length == 0
                || (r.buffer_row_length >= e.width && r.buffer_row_length as u64 % bw == 0))
                && (r.buffer_image_height == 0
                    || (r.buffer_image_height >= e.height
                        && r.buffer_image_height as u64 % bh == 0)),
            "invalid image-copy pitch"
        );
        let row = if r.buffer_row_length == 0 {
            e.width
        } else {
            r.buffer_row_length
        } as u64;
        let height = if r.buffer_image_height == 0 {
            e.height
        } else {
            r.buffer_image_height
        } as u64;
        let pitch = row
            .div_ceil(bw)
            .checked_mul(bytes)
            .ok_or_else(|| anyhow::anyhow!("copy pitch overflow"))?;
        let slice = pitch
            .checked_mul(height.div_ceil(bh))
            .ok_or_else(|| anyhow::anyhow!("copy slice overflow"))?;
        let planes = e.depth as u64 * s.layer_count as u64;
        let body = pitch
            .checked_mul((e.height as u64).div_ceil(bh) - 1)
            .and_then(|n| n.checked_add((e.width as u64).div_ceil(bw) * bytes));
        let end = slice
            .checked_mul(planes - 1)
            .and_then(|n| n.checked_add(body?))
            .and_then(|n| n.checked_add(r.buffer_offset));
        ensure!(
            end.is_some_and(|end| end <= memory.size),
            "buffer/image copy exceeds memory slice"
        );
        adjusted.push(vk::BufferImageCopy {
            buffer_offset: offset,
            ..*r
        });
    }
    Ok(adjusted)
}

impl Commands {
    pub fn retain_texture(&mut self, texture: &Arc<Texture>) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&texture.gpu, &self.gpu),
            "texture belongs to a different device"
        );
        self.retained.push(texture.clone());
        Ok(())
    }

    pub unsafe fn transition(
        &mut self,
        texture: &Arc<Texture>,
        old: vk::ImageLayout,
        new: vk::ImageLayout,
        before: Access,
        after: Access,
    ) -> Result<()> {
        ensure!(self.rendering.is_none(), "transition inside rendering");
        self.retain_texture(texture)?;
        let barriers = [vk::ImageMemoryBarrier2::default()
            .image(texture.raw)
            .subresource_range(texture.desc.range())
            .old_layout(old)
            .new_layout(new)
            .src_stage_mask(before.stages)
            .src_access_mask(before.access)
            .dst_stage_mask(after.stages)
            .dst_access_mask(after.access)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)];
        unsafe {
            self.gpu.raw.cmd_pipeline_barrier2(
                self.raw,
                &vk::DependencyInfo::default().image_memory_barriers(&barriers),
            );
        }
        Ok(())
    }

    pub unsafe fn initialize(&mut self, texture: &Arc<Texture>) -> Result<()> {
        unsafe {
            self.transition(
                texture,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
                Access::NONE,
                Access::ALL,
            )
        }
    }

    pub fn clear_color(&mut self, texture: &Arc<Texture>, color: [f32; 4]) -> Result<()> {
        ensure!(
            self.rendering.is_none()
                && texture
                    .desc
                    .usage
                    .contains(vk::ImageUsageFlags::TRANSFER_DST)
                && texture.desc.aspect() == vk::ImageAspectFlags::COLOR,
            "invalid color clear"
        );
        self.retain_texture(texture)?;
        unsafe {
            self.gpu.raw.cmd_clear_color_image(
                self.raw,
                texture.raw,
                vk::ImageLayout::GENERAL,
                &vk::ClearColorValue { float32: color },
                &[texture.desc.range()],
            );
        }
        Ok(())
    }

    pub unsafe fn upload_image(
        &mut self,
        source: &MemorySlice,
        destination: &Arc<Texture>,
        regions: &[vk::BufferImageCopy],
    ) -> Result<()> {
        ensure!(
            self.rendering.is_none()
                && destination
                    .desc
                    .usage
                    .contains(vk::ImageUsageFlags::TRANSFER_DST),
            "invalid image upload"
        );
        let regions = checked_regions(destination, source, regions)?;
        self.retain(source)?;
        self.retain_texture(destination)?;
        unsafe {
            self.gpu.raw.cmd_copy_buffer_to_image(
                self.raw,
                source.memory.raw,
                destination.raw,
                vk::ImageLayout::GENERAL,
                &regions,
            );
        }
        Ok(())
    }

    pub unsafe fn read_image(
        &mut self,
        source: &Arc<Texture>,
        destination: &MemorySlice,
        regions: &[vk::BufferImageCopy],
    ) -> Result<()> {
        ensure!(
            self.rendering.is_none()
                && source
                    .desc
                    .usage
                    .contains(vk::ImageUsageFlags::TRANSFER_SRC),
            "invalid image readback"
        );
        let regions = checked_regions(source, destination, regions)?;
        self.retain(destination)?;
        self.retain_texture(source)?;
        unsafe {
            self.gpu.raw.cmd_copy_image_to_buffer(
                self.raw,
                source.raw,
                vk::ImageLayout::GENERAL,
                destination.memory.raw,
                &regions,
            );
        }
        Ok(())
    }

    pub fn clear_depth(&mut self, texture: &Arc<Texture>, depth: f32, stencil: u32) -> Result<()> {
        ensure!(
            self.rendering.is_none()
                && texture
                    .desc
                    .usage
                    .contains(vk::ImageUsageFlags::TRANSFER_DST)
                && texture.desc.aspect() != vk::ImageAspectFlags::COLOR
                && (0.0..=1.0).contains(&depth),
            "invalid depth clear"
        );
        self.retain_texture(texture)?;
        unsafe {
            self.gpu.raw.cmd_clear_depth_stencil_image(
                self.raw,
                texture.raw,
                vk::ImageLayout::GENERAL,
                &vk::ClearDepthStencilValue { depth, stencil },
                &[texture.desc.range()],
            );
        }
        Ok(())
    }

    pub fn copy_texture(
        &mut self,
        source: &Arc<Texture>,
        destination: &Arc<Texture>,
    ) -> Result<()> {
        ensure!(
            self.rendering.is_none() && source.raw != destination.raw,
            "invalid texture copy scope or overlapping images"
        );
        let a = source.desc;
        let b = destination.desc;
        ensure!(
            a.format == b.format
                && a.extent == b.extent
                && a.layers == b.layers
                && a.mip_levels == b.mip_levels
                && a.samples == b.samples
                && a.kind == b.kind,
            "incompatible texture copy"
        );
        ensure!(
            a.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                && b.usage.contains(vk::ImageUsageFlags::TRANSFER_DST),
            "missing image-copy usage"
        );
        let mut regions = Vec::new();
        for mip in 0..a.mip_levels {
            for aspect in [
                vk::ImageAspectFlags::COLOR,
                vk::ImageAspectFlags::DEPTH,
                vk::ImageAspectFlags::STENCIL,
            ] {
                if !a.aspect().contains(aspect) {
                    continue;
                }
                let layers = vk::ImageSubresourceLayers::default()
                    .aspect_mask(aspect)
                    .mip_level(mip)
                    .base_array_layer(0)
                    .layer_count(a.layers);
                regions.push(
                    vk::ImageCopy::default()
                        .src_subresource(layers)
                        .dst_subresource(layers)
                        .extent(vk::Extent3D {
                            width: (a.extent.width >> mip).max(1),
                            height: (a.extent.height >> mip).max(1),
                            depth: (a.extent.depth >> mip).max(1),
                        }),
                );
            }
        }
        self.retain_texture(source)?;
        self.retain_texture(destination)?;
        unsafe {
            self.gpu.raw.cmd_copy_image(
                self.raw,
                source.raw,
                vk::ImageLayout::GENERAL,
                destination.raw,
                vk::ImageLayout::GENERAL,
                &regions,
            );
        }
        Ok(())
    }
}
