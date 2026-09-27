use anyhow::{Context, Result, ensure};
use ash::vk;
use ash::vk::TaggedStructure;
use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphicsMode {
    #[default]
    Full,
    Capture,
}

impl GraphicsMode {
    pub fn from_env() -> Result<Self> {
        match std::env::var("ZENITH_GRAPHICS_MODE").as_deref() {
            Ok("full") | Err(std::env::VarError::NotPresent) => Ok(Self::Full),
            Ok("capture") => Ok(Self::Capture),
            _ => anyhow::bail!("ZENITH_GRAPHICS_MODE must be full or capture"),
        }
    }
}

pub(crate) const IMAGES: u32 = 16384;
pub(crate) const SAMPLERS: u32 = 256;

pub(crate) struct CaptureLayout {
    device: ash::Device,
    pub set: vk::DescriptorSetLayout,
    pub pipeline: vk::PipelineLayout,
}

impl CaptureLayout {
    pub fn new(device: &ash::Device) -> Result<Self> {
        let bindings = [
            (0, vk::DescriptorType::SAMPLER, SAMPLERS),
            (2, vk::DescriptorType::SAMPLED_IMAGE, IMAGES),
            (3, vk::DescriptorType::STORAGE_IMAGE, IMAGES),
        ]
        .map(|(binding, ty, count)| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(ty)
                .descriptor_count(count)
                .stage_flags(vk::ShaderStageFlags::ALL)
        });
        let flags = [vk::DescriptorBindingFlags::PARTIALLY_BOUND
            | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
            | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING; 3];
        let mut binding_flags =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        let set = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(&bindings)
                    .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                    .push(&mut binding_flags),
                None,
            )?
        };
        let sets = [set];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::ALL)
            .size(16)];
        let pipeline = match unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sets)
                    .push_constant_ranges(&ranges),
                None,
            )
        } {
            Ok(layout) => layout,
            Err(error) => {
                unsafe {
                    device.destroy_descriptor_set_layout(set, None);
                }
                return Err(error.into());
            }
        };
        Ok(Self {
            device: device.clone(),
            set,
            pipeline,
        })
    }
}

impl Drop for CaptureLayout {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline_layout(self.pipeline, None);
            self.device.destroy_descriptor_set_layout(self.set, None);
        }
    }
}

pub(crate) struct CaptureDescriptors {
    device: ash::Device,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    writes: Mutex<()>,
}

impl CaptureDescriptors {
    pub fn new(gpu: &super::Gpu, images: u32, samplers: u32) -> Result<Self> {
        ensure!(
            images > 0 && images <= IMAGES && samplers > 0 && samplers <= SAMPLERS,
            "capture descriptor capacity exceeds {IMAGES} images or {SAMPLERS} samplers"
        );
        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: SAMPLERS,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: IMAGES,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: IMAGES,
            },
        ];
        let layouts = [gpu
            .capture
            .as_ref()
            .context("capture layout unavailable")?
            .set];
        let pool = unsafe {
            gpu.raw.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes)
                    .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND),
                None,
            )?
        };
        let set = match unsafe {
            gpu.raw.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        } {
            Ok(sets) => sets[0],
            Err(error) => {
                unsafe {
                    gpu.raw.destroy_descriptor_pool(pool, None);
                }
                return Err(error.into());
            }
        };
        Ok(Self {
            device: gpu.raw.clone(),
            pool,
            set,
            writes: Mutex::new(()),
        })
    }

    pub fn image(&self, index: u32, view: vk::ImageView, storage: bool) {
        let _lock = self.writes.lock();
        let images = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(if storage { 3 } else { 2 })
            .dst_array_element(index)
            .descriptor_type(if storage {
                vk::DescriptorType::STORAGE_IMAGE
            } else {
                vk::DescriptorType::SAMPLED_IMAGE
            })
            .image_info(&images);
        unsafe {
            self.device.update_descriptor_sets(&[write], &[]);
        }
    }

    pub fn sampler(&self, index: u32, sampler: vk::Sampler) {
        let _lock = self.writes.lock();
        let images = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .dst_array_element(index)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&images);
        unsafe {
            self.device.update_descriptor_sets(&[write], &[]);
        }
    }
}

impl Drop for CaptureDescriptors {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_descriptor_pool(self.pool, None);
        }
    }
}
