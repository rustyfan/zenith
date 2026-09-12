use super::{Access, Commands, Gpu, Instance, Submission, Texture, TextureDesc};
use anyhow::{Result, ensure};
use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use winit::window::Window;
use zenith_core::log;

struct Surface {
    _instance: Arc<Instance>,
    api: ash::khr::surface::Instance,
    raw: vk::SurfaceKHR,
    window: Arc<Window>,
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            self.api.destroy_surface(self.raw, None);
        }
    }
}

struct Binary {
    gpu: Arc<Gpu>,
    raw: vk::Semaphore,
}
impl Binary {
    fn new(gpu: &Arc<Gpu>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            gpu: gpu.clone(),
            raw: unsafe {
                gpu.raw
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?
            },
        }))
    }
}
impl Drop for Binary {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_semaphore(self.raw, None);
        }
    }
}

struct SwapchainOwner {
    gpu: Arc<Gpu>,
    _surface: Arc<Surface>,
    api: ash::khr::swapchain::Device,
    raw: vk::SwapchainKHR,
    complete: Vec<Arc<Binary>>,
    invalid: AtomicBool,
}

impl Drop for SwapchainOwner {
    fn drop(&mut self) {
        let _ = self.gpu.wait_idle();
        unsafe {
            self.api.destroy_swapchain(self.raw, None);
        }
    }
}

pub struct Swapchain {
    gpu: Arc<Gpu>,
    surface: Arc<Surface>,
    owner: Option<Arc<SwapchainOwner>>,
    images: Vec<Arc<Texture>>,
    extent: vk::Extent2D,
    format: vk::Format,
}

impl Swapchain {
    pub fn new(window: Arc<Window>, validation: bool) -> Result<Self> {
        let display = window.display_handle()?.as_raw();
        let extensions: Vec<_> = ash_window::enumerate_required_extensions(display)?
            .iter()
            .map(|name| unsafe { std::ffi::CStr::from_ptr(*name) })
            .collect();
        let instance = Instance::new(&extensions, validation)?;
        let api = ash::khr::surface::Instance::load(&instance.entry, &instance.raw);
        let factory = ash_window::SurfaceFactory::new(&instance.entry, &instance.raw, display)?;
        let raw = unsafe { factory.create_surface(window.window_handle()?.as_raw(), None)? };
        let surface = Arc::new(Surface {
            _instance: instance.clone(),
            api,
            raw,
            window,
        });
        let gpu = Gpu::create(
            instance,
            std::env::var("ZENITH_ADAPTER").ok().as_deref(),
            Some(raw),
        )?;
        let mut swapchain = Self {
            gpu,
            surface,
            owner: None,
            images: Vec::new(),
            extent: vk::Extent2D::default(),
            format: vk::Format::UNDEFINED,
        };
        swapchain.resize()?;
        Ok(swapchain)
    }

    pub fn gpu(&self) -> &Arc<Gpu> {
        &self.gpu
    }
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }
    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn resize(&mut self) -> Result<()> {
        let size = self.surface.window.inner_size();
        if size.width == 0 || size.height == 0 {
            self.extent = vk::Extent2D::default();
            return Ok(());
        }
        let caps = unsafe {
            self.surface
                .api
                .get_physical_device_surface_capabilities(self.gpu.physical, self.surface.raw)?
        };
        let formats = unsafe {
            self.surface
                .api
                .get_physical_device_surface_formats(self.gpu.physical, self.surface.raw)?
        };
        let format = formats
            .iter()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_SRGB
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .or_else(|| formats.first())
            .copied()
            .ok_or_else(|| anyhow::anyhow!("surface has no formats"))?;
        let format = if format.format == vk::Format::UNDEFINED {
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_SRGB,
                color_space: format.color_space,
            }
        } else {
            format
        };
        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: size
                    .width
                    .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: size
                    .height
                    .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };
        if extent.width == 0 || extent.height == 0 {
            self.extent = extent;
            return Ok(());
        }
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST;
        ensure!(
            caps.supported_usage_flags.contains(usage),
            "surface does not support rendering and transfer clears"
        );
        let mut count = caps.min_image_count.max(3);
        if caps.max_image_count != 0 {
            count = count.min(caps.max_image_count);
        }
        let alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ]
        .into_iter()
        .find(|a| caps.supported_composite_alpha.contains(*a))
        .ok_or_else(|| anyhow::anyhow!("surface has no composite alpha mode"))?;
        let api = ash::khr::swapchain::Device::load(&self.gpu.instance.raw, &self.gpu.raw);
        let modes = unsafe {
            self.surface
                .api
                .get_physical_device_surface_present_modes(self.gpu.physical, self.surface.raw)?
        };
        let present_mode = if modes.contains(&vk::PresentModeKHR::MAILBOX) {
            vk::PresentModeKHR::MAILBOX
        } else {
            vk::PresentModeKHR::FIFO
        };
        let create = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface.raw)
            .min_image_count(count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(alpha)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(
                self.owner
                    .as_ref()
                    .map_or(vk::SwapchainKHR::null(), |o| o.raw),
            );
        let raw = unsafe { api.create_swapchain(&create, None)? };
        let mut owner = SwapchainOwner {
            gpu: self.gpu.clone(),
            _surface: self.surface.clone(),
            api,
            raw,
            complete: Vec::new(),
            invalid: AtomicBool::new(false),
        };
        let raws = unsafe { owner.api.get_swapchain_images(raw)? };
        for _ in &raws {
            owner.complete.push(Binary::new(&self.gpu)?);
        }
        let owner = Arc::new(owner);
        let desc = TextureDesc {
            usage,
            ..TextureDesc::color(extent.width, extent.height, format.format)
        };
        let images = raws
            .into_iter()
            .map(|raw| {
                Arc::new(Texture {
                    gpu: self.gpu.clone(),
                    raw,
                    allocation: None,
                    desc,
                    _owner: Some(owner.clone()),
                })
            })
            .collect();
        self.images = images;
        self.owner = Some(owner);
        self.extent = extent;
        self.format = format.format;
        Ok(())
    }

    pub fn acquire(&mut self) -> Result<Option<Frame>> {
        let size = self.surface.window.inner_size();
        if size.width == 0 || size.height == 0 || self.surface.window.is_minimized() == Some(true) {
            return Ok(None);
        }
        if self
            .owner
            .as_ref()
            .is_none_or(|owner| owner.invalid.load(Ordering::Acquire))
            || self.extent.width != size.width
            || self.extent.height != size.height
        {
            self.resize()?;
        }
        if self.extent.width == 0 || self.extent.height == 0 {
            return Ok(None);
        }
        let owner = self.owner.as_ref().unwrap();
        let acquired = Binary::new(&self.gpu)?;
        match unsafe {
            owner
                .api
                .acquire_next_image(owner.raw, 1_000_000_000, acquired.raw, vk::Fence::null())
        } {
            Ok((index, suboptimal)) => {
                if suboptimal {
                    owner.invalid.store(true, Ordering::Release);
                }
                Ok(Some(Frame {
                    texture: self.images[index as usize].clone(),
                    owner: owner.clone(),
                    acquired,
                    index,
                    consumed: false,
                }))
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                owner.invalid.store(true, Ordering::Release);
                Ok(None)
            }
            Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

pub struct Frame {
    texture: Arc<Texture>,
    owner: Arc<SwapchainOwner>,
    acquired: Arc<Binary>,
    index: u32,
    consumed: bool,
}

impl Frame {
    pub fn texture(&self) -> &Arc<Texture> {
        &self.texture
    }
    pub fn present(mut self, mut commands: Commands) -> Result<Submission> {
        ensure!(commands.queue == 0, "presentation must use queue zero");
        ensure!(
            Arc::ptr_eq(&commands.gpu, &self.owner.gpu),
            "frame and commands belong to different devices"
        );
        unsafe {
            commands.transition(
                &self.texture,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::PRESENT_SRC_KHR,
                Access::ALL,
                Access::NONE,
            )?;
        }
        commands.retained.push(self.acquired.clone());
        commands.retained.push(self.owner.clone());
        let wait = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.acquired.raw)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let signal = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.owner.complete[self.index as usize].raw)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let submission = commands.submit_with(&wait, &signal)?;
        self.consumed = true;
        let complete = [self.owner.complete[self.index as usize].raw];
        let chains = [self.owner.raw];
        let indices = [self.index];
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(&complete)
            .swapchains(&chains)
            .image_indices(&indices);
        let _queue = self.owner.gpu.queues[0]
            .submitted
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match unsafe {
            self.owner
                .api
                .queue_present(self.owner.gpu.queues[0].raw, &present)
        } {
            Ok(suboptimal) => {
                if suboptimal {
                    self.owner.invalid.store(true, Ordering::Release);
                }
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.owner.invalid.store(true, Ordering::Release);
            }
            Err(error) => return Err(error.into()),
        }
        Ok(submission)
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if !self.consumed {
            self.owner.invalid.store(true, Ordering::Release);
            let release = || -> Result<()> {
                let mut commands = self.owner.gpu.commands()?;
                commands.retained.push(self.acquired.clone());
                let waits = [vk::SemaphoreSubmitInfo::default()
                    .semaphore(self.acquired.raw)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
                commands.submit_with(&waits, &[])?.wait(10_000_000_000)
            };
            if let Err(error) = release() {
                log::error!("abandoned frame cleanup failed: {error}");
            }
        }
    }
}
