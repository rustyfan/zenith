use anyhow::{ensure, Context, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use zenith_asset::{
    mesh::{Mesh, VertexLayout},
    texture::{Texture as CpuTexture, TextureFormat},
    Asset, AssetId, AssetSnapshot, CpuRetention, Handle, Revision,
};
use zenith_rhi::*;

type Key = (AssetId, Revision);
pub(crate) struct Geometry {
    pub vertices: Arc<Memory>,
    pub indices: Arc<Memory>,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct AssetUploadStats {
    pub meshes: u64,
    pub textures: u64,
    pub bytes: u64,
    pub staging_allocations: u64,
}
pub(crate) struct GpuAssets {
    gpu: Arc<Gpu>,
    descriptors: Arc<Descriptors>,
    meshes: HashMap<Key, Weak<Geometry>>,
    textures: HashMap<Key, Weak<ImageBinding>>,
    free: Vec<Arc<Memory>>,
    staging_cache_budget: u64,
    stats: AssetUploadStats,
}
impl GpuAssets {
    pub fn new(gpu: &Arc<Gpu>, descriptors: &Arc<Descriptors>) -> Self {
        Self {
            gpu: gpu.clone(),
            descriptors: descriptors.clone(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            free: Vec::new(),
            staging_cache_budget: 128 * 1024 * 1024,
            stats: AssetUploadStats::default(),
        }
    }
    pub fn check_device(&self, gpu: &Arc<Gpu>, descriptors: &Arc<Descriptors>) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.gpu, gpu) && Arc::ptr_eq(&self.descriptors, descriptors),
            "asset cache belongs to another GPU/descriptor heap"
        );
        Ok(())
    }
    pub fn stats(&self) -> AssetUploadStats {
        self.stats
    }
    pub fn set_staging_cache_budget(&mut self, bytes: u64) {
        self.staging_cache_budget = bytes;
        let mut size = 0;
        self.free.retain(|buffer| {
            size += buffer.size();
            size <= bytes
        });
    }
    pub fn cached_mesh<V: VertexLayout>(&self, handle: &Handle<Mesh<V>>) -> Option<Arc<Geometry>> {
        self.meshes
            .get(&(handle.id(), handle.revision()?))?
            .upgrade()
    }
    pub fn cached_texture(&self, handle: &Handle<CpuTexture>) -> Option<Arc<ImageBinding>> {
        self.textures
            .get(&(handle.id(), handle.revision()?))?
            .upgrade()
    }
    pub fn begin(&mut self) -> Result<Upload> {
        self.meshes.retain(|_, value| value.strong_count() > 0);
        self.textures.retain(|_, value| value.strong_count() > 0);
        Ok(Upload {
            gpu: self.gpu.clone(),
            commands: self.gpu.commands()?,
            free: std::mem::take(&mut self.free),
            used: Vec::new(),
            offset: 0,
            bytes: 0,
            allocations: 0,
            acknowledgements: HashMap::new(),
        })
    }
    pub fn mesh<V: VertexLayout>(
        &mut self,
        upload: &mut Upload,
        handle: &Handle<Mesh<V>>,
    ) -> Result<Arc<Geometry>> {
        if let Some(mesh) = self.cached_mesh(handle) {
            if let Some(snapshot) = handle.snapshot() {
                upload.consumed(handle, &snapshot);
            }
            return Ok(mesh);
        }
        let snapshot = handle.snapshot().context("mesh is not loaded")?;
        let key = (handle.id(), snapshot.revision);
        snapshot.validate()?;
        let mesh = Arc::new(Geometry {
            vertices: upload.buffer(snapshot.vertices_bytes())?,
            indices: upload.buffer(snapshot.indices_bytes())?,
        });
        self.meshes.insert(key, Arc::downgrade(&mesh));
        self.stats.meshes += 1;
        upload.consumed(handle, &snapshot);
        Ok(mesh)
    }
    pub fn texture(
        &mut self,
        upload: &mut Upload,
        handle: &Handle<CpuTexture>,
    ) -> Result<Arc<ImageBinding>> {
        if let Some(texture) = self.cached_texture(handle) {
            if let Some(snapshot) = handle.snapshot() {
                upload.consumed(handle, &snapshot);
            }
            return Ok(texture);
        }
        let snapshot = handle.snapshot().context("texture is not loaded")?;
        let key = (handle.id(), snapshot.revision);
        let texture = upload.texture(&snapshot)?;
        let binding = self.descriptors.image(&texture.full_view()?, false)?;
        self.textures.insert(key, Arc::downgrade(&binding));
        self.stats.textures += 1;
        upload.consumed(handle, &snapshot);
        Ok(binding)
    }
    pub fn submit(&mut self, mut upload: Upload) -> Result<UploadTicket> {
        self.stats.bytes += upload.bytes;
        self.stats.staging_allocations += upload.allocations;
        let submission = if upload.bytes > 0 {
            upload.commands.barrier(Access::COPY_WRITE, Access::ALL)?;
            Some(upload.commands.submit()?)
        } else {
            None
        };
        Ok(UploadTicket {
            submission,
            buffers: upload.used.into_iter().chain(upload.free).collect(),
            acknowledgements: upload.acknowledgements,
        })
    }
    pub fn recycle(&mut self, mut ticket: UploadTicket) {
        let mut size: u64 = self.free.iter().map(|buffer| buffer.size()).sum();
        for buffer in ticket.buffers.drain(..) {
            if size + buffer.size() <= self.staging_cache_budget {
                size += buffer.size();
                self.free.push(buffer);
            }
        }
    }
}
pub(crate) struct Upload {
    gpu: Arc<Gpu>,
    commands: Commands,
    free: Vec<Arc<Memory>>,
    used: Vec<Arc<Memory>>,
    offset: u64,
    bytes: u64,
    allocations: u64,
    acknowledgements: HashMap<(Key, usize), Box<dyn FnOnce() + Send>>,
}
impl Upload {
    pub fn consumed<T: Asset>(&mut self, handle: &Handle<T>, snapshot: &AssetSnapshot<T>) {
        if handle.cpu_retention() == CpuRetention::ReleaseAfterUpload {
            let key = (
                (handle.id(), snapshot.revision),
                Arc::as_ptr(&snapshot.value) as usize,
            );
            self.acknowledgements.entry(key).or_insert_with(|| {
                let handle = handle.clone();
                let snapshot = snapshot.clone();
                Box::new(move || {
                    handle.release_cpu(&snapshot);
                })
            });
        }
    }
    fn stage(&mut self, bytes: &[u8]) -> Result<MemorySlice> {
        let length = bytes.len() as u64;
        ensure!(length > 0, "empty asset upload");
        let alignment = self
            .used
            .last()
            .map_or(16, |buffer| buffer.host_write_alignment().max(16));
        self.offset = (self.offset + alignment - 1) & !(alignment - 1);
        if self
            .used
            .last()
            .is_none_or(|buffer| self.offset + length > buffer.size())
        {
            let buffer =
                if let Some(index) = self.free.iter().position(|buffer| buffer.size() >= length) {
                    self.free.swap_remove(index)
                } else {
                    self.allocations += 1;
                    self.gpu
                        .allocate(length.max(16 * 1024 * 1024), MemoryDomain::Upload)?
                };
            self.used.push(buffer);
            self.offset = 0;
        }
        let buffer = self.used.last().unwrap();
        buffer.write(self.offset, bytes)?;
        let slice = buffer.slice(self.offset..self.offset + length)?;
        self.offset += length;
        self.bytes += length;
        Ok(slice)
    }
    fn buffer(&mut self, bytes: &[u8]) -> Result<Arc<Memory>> {
        let staging = self.stage(bytes)?;
        let memory = self
            .gpu
            .allocate(bytes.len() as u64, MemoryDomain::Device)?;
        self.commands.copy(&staging, &memory.whole())?;
        Ok(memory)
    }
    fn texture(&mut self, asset: &CpuTexture) -> Result<Arc<Texture>> {
        asset.validate()?;
        let mut desc = TextureDesc::color(asset.width, asset.height, texture_format(asset.format));
        desc.usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
        desc.cube = asset.is_cubemap;
        desc.layers = if asset.is_cubemap { 6 } else { 1 };
        desc.mip_levels = asset.mip_levels;
        let texture = self.gpu.texture(desc)?;
        unsafe {
            self.commands.initialize(&texture)?;
        }
        let mut offset = 0;
        for mip in 0..desc.mip_levels {
            let width = (asset.width >> mip).max(1);
            let height = (asset.height >> mip).max(1);
            let length = asset.format.data_size_in_bytes(width, height) * desc.layers as usize;
            let staging = self.stage(&asset.pixels[offset..offset + length])?;
            let region = vk::BufferImageCopy::default()
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(mip)
                        .layer_count(desc.layers),
                );
            unsafe {
                self.commands.upload_image(&staging, &texture, &[region])?;
            }
            offset += length;
        }
        Ok(texture)
    }
}
pub(crate) struct UploadTicket {
    submission: Option<Submission>,
    buffers: Vec<Arc<Memory>>,
    acknowledgements: HashMap<(Key, usize), Box<dyn FnOnce() + Send>>,
}
impl UploadTicket {
    pub fn release_cpu(&mut self) {
        for (_, acknowledge) in self.acknowledgements.drain() {
            acknowledge();
        }
    }
    pub fn poll(&mut self) -> Result<bool> {
        self.submission.as_mut().map_or(Ok(true), Submission::poll)
    }
    pub fn wait(&mut self) -> Result<()> {
        if let Some(submission) = &mut self.submission {
            submission.wait(10_000_000_000)?;
        }
        Ok(())
    }
}
fn texture_format(format: TextureFormat) -> vk::Format {
    match format {
        TextureFormat::R8Unorm => vk::Format::R8_UNORM,
        TextureFormat::Rg8Unorm => vk::Format::R8G8_UNORM,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        TextureFormat::R16Unorm => vk::Format::R16_UNORM,
        TextureFormat::Rg16Unorm => vk::Format::R16G16_UNORM,
        TextureFormat::Rgba16Unorm => vk::Format::R16G16B16A16_UNORM,
        TextureFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        TextureFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        TextureFormat::Bc5Unorm => vk::Format::BC5_UNORM_BLOCK,
        TextureFormat::Bc7Unorm => vk::Format::BC7_UNORM_BLOCK,
        TextureFormat::Bc7Srgb => vk::Format::BC7_SRGB_BLOCK,
        TextureFormat::Bc6hUfloat => vk::Format::BC6H_UFLOAT_BLOCK,
        TextureFormat::Bc6hSfloat => vk::Format::BC6H_SFLOAT_BLOCK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use zenith_asset::{mesh::Vertex, AssetServer, ImportContext, Importer, MemorySource};

    struct Pixels;
    impl Importer for Pixels {
        type Settings = ();
        type Output = CpuTexture;
        const KEY: &'static str = "test.pixels";
        const VERSION: u32 = 1;
        fn extensions(&self) -> &[&str] {
            &["pixels"]
        }
        fn import(
            &self,
            bytes: &[u8],
            _: &(),
            _: &mut ImportContext<'_>,
        ) -> zenith_asset::Result<CpuTexture> {
            Ok(CpuTexture {
                width: 3,
                height: 1,
                mip_levels: 2,
                is_cubemap: false,
                format: TextureFormat::R8Unorm,
                pixels: vec![bytes[0]; 4],
            })
        }
    }
    #[test]
    #[ignore = "requires Vulkan validation and an address/descriptor-heap capable GPU"]
    fn upload_acknowledgements_preserve_replacements_and_release_completed_staging() -> Result<()> {
        let instance = Instance::new(&[], true)?;
        ensure!(
            instance.validation_enabled(),
            "GPU asset test requires Vulkan validation"
        );
        let gpu = Gpu::new(
            instance.clone(),
            std::env::var("ZENITH_ADAPTER").ok().as_deref(),
        )?;
        {
            let descriptors = Descriptors::new(&gpu, 64, 8)?;
            let mut cache = GpuAssets::new(&gpu, &descriptors);
            cache.set_staging_cache_budget(0);
            let source = Arc::new(MemorySource::default());
            source.insert("a.pixels", vec![100])?;
            let server = AssetServer::builder()
                .source(source.clone())
                .register_asset::<CpuTexture>()
                .register_importer(Pixels)
                .cpu_retention::<CpuTexture>(CpuRetention::ReleaseAfterUpload)
                .build()?;
            let handle = server.load_blocking::<CpuTexture>("a.pixels")?;
            let mut upload = cache.begin()?;
            let first = cache.texture(&mut upload, &handle)?;
            cache.texture(&mut upload, &handle)?;
            assert_eq!(upload.acknowledgements.len(), 1);
            let mut ticket = cache.submit(upload)?;
            let staging = Arc::downgrade(&ticket.buffers[0]);
            assert!(handle.get().is_some());
            source.insert("a.pixels", vec![200])?;
            server.reload(&handle)?;
            handle.wait()?;
            ticket.wait()?;
            ticket.release_cpu();
            cache.recycle(ticket);
            assert!(cache.free.is_empty());
            assert!(staging.upgrade().is_none());
            assert_eq!(handle.get().unwrap().pixels[0], 200);

            let snapshot = handle.snapshot().unwrap();
            let mut upload = cache.begin()?;
            let second = cache.texture(&mut upload, &handle)?;
            let mut ticket = cache.submit(upload)?;
            assert!(handle.release_cpu(&snapshot));
            server.reload(&handle)?;
            handle.wait()?;
            assert_eq!(handle.revision(), Some(snapshot.revision));
            assert!(!Arc::ptr_eq(&handle.get().unwrap(), &snapshot.value));
            ticket.wait()?;
            ticket.release_cpu();
            cache.recycle(ticket);
            assert!(handle.get().is_some());

            let mut upload = cache.begin()?;
            assert!(Arc::ptr_eq(&second, &cache.texture(&mut upload, &handle)?));
            let mut cancelled = cache.submit(upload)?;
            cancelled.wait()?;
            cache.recycle(cancelled);
            assert!(handle.get().is_some());
            let mut upload = cache.begin()?;
            cache.texture(&mut upload, &handle)?;
            let mut ticket = cache.submit(upload)?;
            ticket.wait()?;
            ticket.release_cpu();
            cache.recycle(ticket);
            assert!(handle.get().is_none());
            assert!(!Arc::ptr_eq(
                first.view().texture(),
                second.view().texture()
            ));
        }
        gpu.wait_idle()?;
        drop(gpu);
        ensure!(
            instance.validation_errors().is_empty(),
            "GPU validation errors: {:?}",
            instance.validation_errors()
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires Vulkan validation and an address/descriptor-heap capable GPU"]
    fn uploads_deduplicate_recycle_and_keep_old_revisions_alive() -> Result<()> {
        let instance = Instance::new(&[], true)?;
        ensure!(
            instance.validation_enabled(),
            "GPU asset test requires Vulkan validation"
        );
        let gpu = Gpu::new(
            instance.clone(),
            std::env::var("ZENITH_ADAPTER").ok().as_deref(),
        )?;
        {
            let descriptors = Descriptors::new(&gpu, 64, 8)?;
            let mut cache = GpuAssets::new(&gpu, &descriptors);
            let source = Arc::new(MemorySource::default());
            source.insert("a.pixels", vec![100])?;
            let server = AssetServer::builder()
                .source(source.clone())
                .register_asset::<CpuTexture>()
                .register_importer(Pixels)
                .build()?;
            let texture = server.load_blocking::<CpuTexture>("a.pixels")?;
            let mesh = server.add(Mesh::new(
                vec![
                    Vertex {
                        position: [0.0, 0.0, 0.0],
                        normal: [0.0, 0.0, 1.0],
                        tex_coord: [0.0; 2],
                        tangent: [0.0; 4],
                    },
                    Vertex {
                        position: [1.0, 0.0, 0.0],
                        normal: [0.0, 0.0, 1.0],
                        tex_coord: [0.0; 2],
                        tangent: [0.0; 4],
                    },
                    Vertex {
                        position: [0.0, 1.0, 0.0],
                        normal: [0.0, 0.0, 1.0],
                        tex_coord: [0.0; 2],
                        tangent: [0.0; 4],
                    },
                ],
                vec![0, 1, 2],
            ));
            let mut upload = cache.begin()?;
            let geometry = cache.mesh(&mut upload, &mesh)?;
            let first = cache.texture(&mut upload, &texture)?;
            assert!(Arc::ptr_eq(&geometry, &cache.mesh(&mut upload, &mesh)?));
            assert!(Arc::ptr_eq(&first, &cache.texture(&mut upload, &texture)?));
            let mut ticket = cache.submit(upload)?;
            let started = Instant::now();
            while !ticket.poll()? {
                ensure!(
                    started.elapsed() < Duration::from_secs(10),
                    "asset upload timed out"
                );
                std::thread::yield_now();
            }
            cache.recycle(ticket);
            let before = cache.stats();
            assert_eq!(
                (before.meshes, before.textures, before.staging_allocations),
                (1, 1, 1)
            );
            let mut upload = cache.begin()?;
            assert!(Arc::ptr_eq(&geometry, &cache.mesh(&mut upload, &mesh)?));
            assert!(Arc::ptr_eq(&first, &cache.texture(&mut upload, &texture)?));
            let mut ticket = cache.submit(upload)?;
            ticket.wait()?;
            cache.recycle(ticket);
            assert_eq!(cache.stats().bytes, before.bytes);
            source.insert("a.pixels", vec![200])?;
            server.reload(&texture)?;
            texture.wait()?;
            let mut upload = cache.begin()?;
            let second = cache.texture(&mut upload, &texture)?;
            assert!(!Arc::ptr_eq(&first, &second));
            let mut ticket = cache.submit(upload)?;
            ticket.wait()?;
            cache.recycle(ticket);
            assert_eq!(cache.stats().textures, 2);
            assert_eq!(cache.stats().staging_allocations, 1);
            let readback = gpu.allocate(geometry.vertices.size(), MemoryDomain::Readback)?;
            let mut commands = gpu.commands()?;
            commands.copy(&geometry.vertices.whole(), &readback.whole())?;
            commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
            commands.submit()?.wait(10_000_000_000)?;
            let mut bytes = vec![0; readback.size() as usize];
            readback.read(0, &mut bytes)?;
            assert_eq!(bytes, mesh.get().unwrap().vertices_bytes());
        }
        gpu.wait_idle()?;
        drop(gpu);
        ensure!(
            instance.validation_errors().is_empty(),
            "GPU asset validation errors: {:?}",
            instance.validation_errors()
        );
        Ok(())
    }
}
