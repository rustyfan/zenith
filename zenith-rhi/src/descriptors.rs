use super::memory::align_up;
use super::{Commands, Gpu, Memory, MemoryDomain, TextureView};
use anyhow::{Context, Result, ensure};
use ash::vk;
use parking_lot::Mutex;
use std::collections::BTreeSet;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn descriptor_stride(gpu: &Gpu, size: u64, alignment: u64) -> Result<u64> {
    align_up(size, alignment.max(gpu.limits.non_coherent_atom_size))
}

impl Gpu {
    pub fn descriptor_strides(&self) -> Result<(u64, u64)> {
        if self.capture.is_some() {
            return Ok((0, 0));
        }
        let p = &self.heap_properties;
        Ok((
            descriptor_stride(self, p.image_descriptor_size, p.image_descriptor_alignment)?,
            descriptor_stride(
                self,
                p.sampler_descriptor_size,
                p.sampler_descriptor_alignment,
            )?,
        ))
    }
}

struct Heap {
    memory: Option<Arc<Memory>>,
    stride: u64,
    capacity: u32,
    reserved: u64,
    free: Mutex<BTreeSet<u32>>,
    writes: AtomicU64,
}

impl Heap {
    fn slots(capacity: u32) -> Self {
        Self {
            memory: None,
            stride: 0,
            capacity,
            reserved: 0,
            free: Mutex::new((0..capacity).collect()),
            writes: AtomicU64::new(0),
        }
    }

    fn new(
        gpu: &Arc<Gpu>,
        capacity: u32,
        size: u64,
        alignment: u64,
        heap_alignment: u64,
        reserved: u64,
        max_size: u64,
    ) -> Result<Self> {
        ensure!(capacity > 0, "empty descriptor heap");
        let stride = descriptor_stride(gpu, size, alignment)?;
        let offset = align_up(
            stride
                .checked_mul(capacity as u64)
                .context("heap size overflow")?,
            heap_alignment,
        )?;
        let size = offset
            .checked_add(reserved)
            .context("heap reservation overflow")?;
        ensure!(
            size <= max_size && offset <= u32::MAX as u64,
            "descriptor heap exceeds device limit"
        );
        let memory = gpu.allocate_buffer(
            size,
            MemoryDomain::Upload,
            vk::BufferUsageFlags::DESCRIPTOR_HEAP_EXT
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST,
            heap_alignment,
        )?;
        Ok(Self {
            memory: Some(memory),
            stride,
            capacity,
            reserved: offset,
            free: Mutex::new((0..capacity).collect()),
            writes: AtomicU64::new(0),
        })
    }

    fn allocate(
        &self,
        write: impl FnOnce(vk::HostAddressRangeEXT<'_>) -> Result<()>,
    ) -> Result<u32> {
        let mut free = self.free.lock();
        let index = free.pop_first().context("descriptor heap exhausted")?;
        drop(free);
        if let Err(error) = self.write(index, write) {
            self.free.lock().insert(index);
            return Err(error);
        }
        Ok(index)
    }

    fn write(
        &self,
        index: u32,
        write: impl FnOnce(vk::HostAddressRangeEXT<'_>) -> Result<()>,
    ) -> Result<()> {
        let offset = self.stride * index as u64;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                (self.memory.as_ref().unwrap().mapped as *mut u8).add(offset as usize),
                self.stride as usize,
            )
        };
        write(vk::HostAddressRangeEXT::default().address(bytes))?;
        self.memory
            .as_ref()
            .unwrap()
            .gpu
            .allocator()
            .flush_allocation(
                &self.memory.as_ref().unwrap().allocation,
                offset,
                self.stride,
            )?;
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn binding(&self) -> vk::BindHeapInfoEXT<'static> {
        vk::BindHeapInfoEXT::default()
            .heap_range(
                vk::DeviceAddressRangeEXT::default()
                    .address(self.memory.as_ref().unwrap().address().value())
                    .size(self.memory.as_ref().unwrap().size()),
            )
            .reserved_range_offset(self.reserved)
            .reserved_range_size(self.memory.as_ref().unwrap().size() - self.reserved)
    }
}

pub struct Descriptors {
    capture: Option<super::capture::CaptureDescriptors>,
    pub(crate) gpu: Arc<Gpu>,
    images: Heap,
    samplers: Heap,
}

impl Descriptors {
    pub fn write_counts(&self) -> (u64, u64) {
        (
            self.images.writes.load(Ordering::Relaxed),
            self.samplers.writes.load(Ordering::Relaxed),
        )
    }
    pub fn new(gpu: &Arc<Gpu>, images: u32, samplers: u32) -> Result<Arc<Self>> {
        if gpu.capture.is_some() {
            return Ok(Arc::new(Self {
                capture: Some(super::capture::CaptureDescriptors::new(
                    gpu, images, samplers,
                )?),
                gpu: gpu.clone(),
                images: Heap::slots(images),
                samplers: Heap::slots(samplers),
            }));
        }
        let p = &gpu.heap_properties;
        Ok(Arc::new(Self {
            capture: None,
            gpu: gpu.clone(),
            images: Heap::new(
                gpu,
                images,
                p.image_descriptor_size,
                p.image_descriptor_alignment,
                p.resource_heap_alignment,
                p.min_resource_heap_reserved_range,
                p.max_resource_heap_size,
            )?,
            samplers: Heap::new(
                gpu,
                samplers,
                p.sampler_descriptor_size,
                p.sampler_descriptor_alignment,
                p.sampler_heap_alignment,
                p.min_sampler_heap_reserved_range,
                p.max_sampler_heap_size,
            )?,
        }))
    }

    pub fn image(
        self: &Arc<Self>,
        view: &Arc<TextureView>,
        storage: bool,
    ) -> Result<Arc<ImageBinding>> {
        zenith_core::profile::scope!("Write image descriptor");
        ensure!(
            Arc::ptr_eq(&self.gpu, &view.texture.gpu),
            "image belongs to another device"
        );
        let usage = if storage {
            vk::ImageUsageFlags::STORAGE
        } else {
            vk::ImageUsageFlags::SAMPLED
        };
        ensure!(
            view.texture.desc.usage.contains(usage),
            "image usage does not support requested descriptor"
        );
        if let Some(capture) = &self.capture {
            let index = self
                .images
                .free
                .lock()
                .pop_first()
                .context("image descriptors exhausted")?;
            capture.image(index, view.raw, storage);
            self.images.writes.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(ImageBinding {
                table: self.clone(),
                view: view.clone(),
                index,
                storage,
            }));
        }
        let create = vk::ImageViewCreateInfo::default()
            .image(view.texture.raw)
            .view_type(view.kind)
            .format(view.texture.desc.format)
            .subresource_range(view.range);
        let image = vk::ImageDescriptorInfoEXT::default()
            .view(&create)
            .layout(vk::ImageLayout::GENERAL);
        let descriptor = vk::ResourceDescriptorInfoEXT::default()
            .ty(if storage {
                vk::DescriptorType::STORAGE_IMAGE
            } else {
                vk::DescriptorType::SAMPLED_IMAGE
            })
            .data(vk::ResourceDescriptorDataEXT { p_image: &image });
        let index = self.images.allocate(|output| {
            unsafe {
                self.gpu
                    .heap
                    .write_resource_descriptors(&[descriptor], &[output])?;
            }
            Ok(())
        })?;
        Ok(Arc::new(ImageBinding {
            table: self.clone(),
            view: view.clone(),
            index,
            storage,
        }))
    }

    pub fn sampler(
        self: &Arc<Self>,
        filter: vk::Filter,
        address: vk::SamplerAddressMode,
    ) -> Result<Arc<Sampler>> {
        self.sampler_with_filters(filter, filter, address)
    }

    pub fn sampler_with_filters(
        self: &Arc<Self>,
        magnification: vk::Filter,
        minification: vk::Filter,
        address: vk::SamplerAddressMode,
    ) -> Result<Arc<Sampler>> {
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(magnification)
            .min_filter(minification)
            .mipmap_mode(if minification == vk::Filter::LINEAR {
                vk::SamplerMipmapMode::LINEAR
            } else {
                vk::SamplerMipmapMode::NEAREST
            })
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .min_lod(0.0)
            .max_lod(vk::LOD_CLAMP_NONE)
            .max_anisotropy(1.0);
        if let Some(capture) = &self.capture {
            let raw = unsafe { self.gpu.raw.create_sampler(&info, None)? };
            let index = match self.samplers.free.lock().pop_first() {
                Some(index) => index,
                None => {
                    unsafe {
                        self.gpu.raw.destroy_sampler(raw, None);
                    }
                    anyhow::bail!("sampler descriptors exhausted");
                }
            };
            capture.sampler(index, raw);
            self.samplers.writes.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(Sampler {
                table: self.clone(),
                index,
                raw: Some(raw),
            }));
        }
        let index = self.samplers.allocate(|output| {
            unsafe {
                self.gpu
                    .heap
                    .write_sampler_descriptors(&[info], &[output])?;
            }
            Ok(())
        })?;
        Ok(Arc::new(Sampler {
            raw: None,
            table: self.clone(),
            index,
        }))
    }

    pub fn available_images(&self) -> usize {
        self.images.free.lock().len()
    }
    pub fn image_capacity(&self) -> u32 {
        self.images.capacity
    }

    pub fn image_array(
        self: &Arc<Self>,
        views: &[Arc<TextureView>],
        storage: bool,
    ) -> Result<Vec<Arc<ImageBinding>>> {
        ensure!(
            !views.is_empty() && views.len() <= self.images.capacity as usize,
            "invalid descriptor array size"
        );
        let usage = if storage {
            vk::ImageUsageFlags::STORAGE
        } else {
            vk::ImageUsageFlags::SAMPLED
        };
        ensure!(
            views
                .iter()
                .all(|v| Arc::ptr_eq(&self.gpu, &v.texture.gpu)
                    && v.texture.desc.usage.contains(usage)),
            "invalid descriptor array resources"
        );
        let mut free = self.images.free.lock();
        let mut run = 0;
        let mut previous = None;
        let mut start = None;
        for &index in free.iter() {
            run = if previous == index.checked_sub(1) {
                run + 1
            } else {
                1
            };
            previous = Some(index);
            if run == views.len() {
                start = Some(index + 1 - run as u32);
                break;
            }
        }
        let start = start.context("no contiguous descriptor range available")?;
        for index in start..start + views.len() as u32 {
            free.remove(&index);
        }
        drop(free);
        let bindings: Vec<_> = views
            .iter()
            .enumerate()
            .map(|(i, view)| {
                Arc::new(ImageBinding {
                    table: self.clone(),
                    view: view.clone(),
                    index: start + i as u32,
                    storage,
                })
            })
            .collect();
        for binding in &bindings {
            if let Some(capture) = &self.capture {
                capture.image(binding.index, binding.view.raw, storage);
                self.images.writes.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let view = &binding.view;
            let create = vk::ImageViewCreateInfo::default()
                .image(view.texture.raw)
                .view_type(view.kind)
                .format(view.texture.desc.format)
                .subresource_range(view.range);
            let image = vk::ImageDescriptorInfoEXT::default()
                .view(&create)
                .layout(vk::ImageLayout::GENERAL);
            let descriptor = vk::ResourceDescriptorInfoEXT::default()
                .ty(if storage {
                    vk::DescriptorType::STORAGE_IMAGE
                } else {
                    vk::DescriptorType::SAMPLED_IMAGE
                })
                .data(vk::ResourceDescriptorDataEXT { p_image: &image });
            self.images.write(binding.index, |output| {
                unsafe {
                    self.gpu
                        .heap
                        .write_resource_descriptors(&[descriptor], &[output])?;
                }
                Ok(())
            })?;
        }
        Ok(bindings)
    }

    pub fn copy_image(
        self: &Arc<Self>,
        commands: &mut Commands,
        source: &Arc<ImageBinding>,
    ) -> Result<Arc<ImageBinding>> {
        ensure!(
            Arc::ptr_eq(&self.gpu, &source.table.gpu)
                && self.images.stride == source.table.images.stride,
            "incompatible descriptor heaps"
        );
        if self.capture.is_some() {
            let destination = self.image(&source.view, source.storage)?;
            commands.retain_image(source)?;
            commands.retain_image(&destination)?;
            return Ok(destination);
        }
        let index = self.images.allocate(|output| {
            unsafe {
                std::slice::from_raw_parts_mut(output.address as *mut u8, output.size).fill(0);
            }
            Ok(())
        })?;
        let destination = Arc::new(ImageBinding {
            table: self.clone(),
            view: source.view.clone(),
            storage: source.storage,
            index,
        });
        let start = self.images.stride * index as u64;
        let source_start = source.table.images.stride * source.index as u64;
        commands.copy(
            &source
                .table
                .images
                .memory
                .as_ref()
                .unwrap()
                .slice(source_start..source_start + self.images.stride)?,
            &self
                .images
                .memory
                .as_ref()
                .unwrap()
                .slice(start..start + self.images.stride)?,
        )?;
        commands.retain_image(source)?;
        commands.retain_image(&destination)?;
        Ok(destination)
    }
}

pub struct ImageBinding {
    storage: bool,
    pub(crate) table: Arc<Descriptors>,
    pub(crate) view: Arc<TextureView>,
    index: u32,
}
impl ImageBinding {
    pub fn index(&self) -> u32 {
        self.index
    }
    pub fn view(&self) -> &Arc<TextureView> {
        &self.view
    }
}
impl Drop for ImageBinding {
    fn drop(&mut self) {
        self.table.images.free.lock().insert(self.index);
    }
}

pub struct Sampler {
    raw: Option<vk::Sampler>,
    pub(crate) table: Arc<Descriptors>,
    index: u32,
}
impl Sampler {
    pub fn index(&self) -> u32 {
        self.index
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        if let Some(raw) = self.raw {
            unsafe {
                self.table.gpu.raw.destroy_sampler(raw, None);
            }
        }
        self.table.samplers.free.lock().insert(self.index);
    }
}

impl Commands {
    pub fn bind_descriptors(&mut self, table: &Arc<Descriptors>) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&table.gpu, &self.gpu),
            "descriptor heaps belong to another device"
        );
        if let Some(capture) = &table.capture {
            let layout = self.gpu.capture.as_ref().unwrap().pipeline;
            for point in [
                vk::PipelineBindPoint::GRAPHICS,
                vk::PipelineBindPoint::COMPUTE,
            ] {
                unsafe {
                    self.gpu.raw.cmd_bind_descriptor_sets(
                        self.raw,
                        point,
                        layout,
                        0,
                        &[capture.set],
                        &[],
                    );
                }
            }
            self.retained.push(table.clone());
            return Ok(());
        }
        unsafe {
            self.gpu
                .heap
                .cmd_bind_resource_heap(self.raw, &table.images.binding());
            self.gpu
                .heap
                .cmd_bind_sampler_heap(self.raw, &table.samplers.binding());
        }
        self.retained.push(table.clone());
        Ok(())
    }
    pub fn retain_image(&mut self, image: &Arc<ImageBinding>) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&image.table.gpu, &self.gpu),
            "descriptor belongs to another device"
        );
        self.retained.push(image.clone());
        Ok(())
    }
    pub fn retain_sampler(&mut self, sampler: &Arc<Sampler>) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&sampler.table.gpu, &self.gpu),
            "sampler belongs to another device"
        );
        self.retained.push(sampler.clone());
        Ok(())
    }
}
