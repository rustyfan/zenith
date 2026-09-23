use anyhow::{Context, Result};
use ash::vk::TaggedStructure;
use ash::{Entry, vk};
use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex};
use zenith_core::log;

#[derive(Debug)]
pub struct AdapterInfo {
    pub cooperative: crate::CooperativeCapabilities,
    pub name: String,
    pub api_version: u32,
    pub driver_version: u32,
    pub driver_name: String,
    pub driver_info: String,
    pub queues: Vec<(vk::QueueFlags, u32)>,
    pub memory_types: Vec<vk::MemoryPropertyFlags>,
    pub image_descriptor_size: u64,
    pub sampler_descriptor_size: u64,
    pub resource_heap_alignment: u64,
    pub sampler_heap_alignment: u64,
    pub descriptor_heap: bool,
    pub shader_untyped_pointers: bool,
    pub unified_image_layouts: bool,
    pub buffer_device_address: bool,
    pub acceleration_structure: bool,
    pub ray_query: bool,
    pub timeline_semaphore: bool,
    pub dynamic_rendering: bool,
    pub synchronization2: bool,
    pub dynamic_blend: bool,
    pub host_visible_device_local: bool,
}

pub struct Instance {
    pub(crate) entry: Entry,
    pub(crate) raw: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    messages: Arc<Mutex<Vec<String>>>,
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user: *mut c_void,
) -> vk::Bool32 {
    if !data.is_null() && !unsafe { (*data).p_message }.is_null() {
        let text = unsafe { CStr::from_ptr((*data).p_message) }
            .to_string_lossy()
            .into_owned();
        if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            log::error!("Vulkan: {text}");
            if let Some(messages) = unsafe { (user as *const Mutex<Vec<String>>).as_ref() } {
                if let Ok(mut messages) = messages.lock() {
                    messages.push(text);
                }
            }
        } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
            log::warn!("Vulkan: {text}");
        }
    }
    vk::FALSE
}

#[cfg(windows)]
fn configure_validation_layer() {
    static CONFIGURE: std::sync::Once = std::sync::Once::new();
    CONFIGURE.call_once(|| {
        if std::env::var_os("VK_LAYER_PATH").is_some()
            || std::env::var_os("VK_ADD_LAYER_PATH").is_some()
        {
            return;
        }
        let sdk =
            std::env::var_os("VULKAN_SDK").map(|path| std::path::PathBuf::from(path).join("Bin"));
        let local =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/vulkan-sdk/Bin");
        for path in sdk.into_iter().chain(std::iter::once(local)) {
            if path.join("VkLayer_khronos_validation.json").is_file()
                && path.join("VkLayer_khronos_validation.dll").is_file()
            {
                // Environment mutation is thread-safe on Windows.
                unsafe { std::env::set_var("VK_LAYER_PATH", path) };
                break;
            }
        }
    });
}

impl Instance {
    pub fn new(extensions: &[&CStr], validation: bool) -> Result<Arc<Self>> {
        #[cfg(windows)]
        if validation {
            configure_validation_layer();
        }
        let entry = unsafe { Entry::load() }.context("Vulkan loader is unavailable")?;
        let validation_name = c"VK_LAYER_KHRONOS_validation";
        let validation_available = validation
            && unsafe { entry.enumerate_instance_layer_properties()? }
                .iter()
                .any(|layer| unsafe {
                    CStr::from_ptr(layer.layer_name.as_ptr()) == validation_name
                });
        if validation && !validation_available {
            log::warn!("Vulkan validation layer unavailable");
        }
        let validation = validation && validation_available;
        let mut enabled: Vec<_> = extensions.iter().map(|name| name.as_ptr()).collect();
        if validation {
            enabled.push(ash::ext::debug_utils::NAME.as_ptr());
        }
        let layer_names = if validation {
            vec![validation_name.as_ptr()]
        } else {
            vec![]
        };
        let application = vk::ApplicationInfo::default()
            .application_name(c"Zenith")
            .api_version(vk::API_VERSION_1_3);
        let messages = Arc::new(Mutex::new(Vec::new()));
        let mut debug_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                    | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
            )
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
            )
            .pfn_user_callback(Some(debug_callback))
            .user_data(Arc::as_ptr(&messages) as *mut c_void);
        let mut info = vk::InstanceCreateInfo::default()
            .application_info(&application)
            .enabled_extension_names(&enabled)
            .enabled_layer_names(&layer_names);
        if validation {
            info = info.push(&mut debug_info);
        }
        let raw = unsafe { entry.create_instance(&info, None)? };
        let debug = if validation {
            let api = ash::ext::debug_utils::Instance::load(&entry, &raw);
            match unsafe { api.create_debug_utils_messenger(&debug_info, None) } {
                Ok(messenger) => Some((api, messenger)),
                Err(error) => {
                    unsafe {
                        raw.destroy_instance(None);
                    }
                    return Err(error.into());
                }
            }
        } else {
            None
        };
        Ok(Arc::new(Self {
            entry,
            raw,
            debug,
            messages,
        }))
    }

    pub fn validation_enabled(&self) -> bool {
        self.debug.is_some()
    }

    pub fn validation_errors(&self) -> Vec<String> {
        self.messages
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn adapters(&self) -> Result<Vec<AdapterInfo>> {
        unsafe { self.raw.enumerate_physical_devices()? }
            .into_iter()
            .map(|physical| self.adapter_info(physical))
            .collect()
    }

    pub(crate) fn adapter_info(&self, physical: vk::PhysicalDevice) -> Result<AdapterInfo> {
        let properties = unsafe { self.raw.get_physical_device_properties(physical) };
        let extensions = unsafe { self.raw.enumerate_device_extension_properties(physical)? };
        let has = |name: &CStr| {
            extensions
                .iter()
                .any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) == name })
        };
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut heap = vk::PhysicalDeviceDescriptorHeapFeaturesEXT::default();
        let mut untyped = vk::PhysicalDeviceShaderUntypedPointersFeaturesKHR::default();
        let mut unified = vk::PhysicalDeviceUnifiedImageLayoutsFeaturesKHR::default();
        let mut dynamic = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default();
        let mut acceleration = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
        let mut ray_query = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .push(&mut f12)
            .push(&mut f13);
        if has(ash::ext::descriptor_heap::NAME) {
            features = features.push(&mut heap);
        }
        if has(ash::khr::shader_untyped_pointers::NAME) {
            features = features.push(&mut untyped);
        }
        if has(ash::khr::unified_image_layouts::NAME) {
            features = features.push(&mut unified);
        }
        if has(ash::ext::extended_dynamic_state3::NAME) {
            features = features.push(&mut dynamic);
        }
        if has(ash::khr::acceleration_structure::NAME) {
            features = features.push(&mut acceleration);
        }
        if has(ash::khr::ray_query::NAME) {
            features = features.push(&mut ray_query);
        }
        unsafe {
            self.raw
                .get_physical_device_features2(physical, &mut features);
        }
        let memory = unsafe { self.raw.get_physical_device_memory_properties(physical) };
        let mut driver = vk::PhysicalDeviceDriverProperties::default();
        let mut sizes = vk::PhysicalDeviceDescriptorHeapPropertiesEXT::default();
        let mut extra = vk::PhysicalDeviceProperties2::default().push(&mut driver);
        if has(ash::ext::descriptor_heap::NAME) {
            extra = extra.push(&mut sizes);
        }
        unsafe {
            self.raw
                .get_physical_device_properties2(physical, &mut extra);
        }
        let queues = unsafe {
            self.raw
                .get_physical_device_queue_family_properties(physical)
        };
        Ok(AdapterInfo {
            cooperative: self.cooperative_capabilities(physical, &extensions)?,
            name: unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
            api_version: properties.api_version,
            driver_version: properties.driver_version,
            driver_name: unsafe { CStr::from_ptr(driver.driver_name.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
            driver_info: unsafe { CStr::from_ptr(driver.driver_info.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
            queues: queues
                .iter()
                .map(|q| (q.queue_flags, q.queue_count))
                .collect(),
            memory_types: memory.memory_types[..memory.memory_type_count as usize]
                .iter()
                .map(|m| m.property_flags)
                .collect(),
            image_descriptor_size: sizes.image_descriptor_size,
            sampler_descriptor_size: sizes.sampler_descriptor_size,
            resource_heap_alignment: sizes.resource_heap_alignment,
            sampler_heap_alignment: sizes.sampler_heap_alignment,
            descriptor_heap: heap.descriptor_heap != 0,
            shader_untyped_pointers: untyped.shader_untyped_pointers != 0,
            unified_image_layouts: unified.unified_image_layouts != 0,
            buffer_device_address: f12.buffer_device_address != 0,
            acceleration_structure: acceleration.acceleration_structure != 0,
            ray_query: ray_query.ray_query != 0,
            timeline_semaphore: f12.timeline_semaphore != 0,
            dynamic_rendering: f13.dynamic_rendering != 0,
            synchronization2: f13.synchronization2 != 0,
            dynamic_blend: dynamic.extended_dynamic_state3_color_blend_enable != 0
                && dynamic.extended_dynamic_state3_color_blend_equation != 0
                && dynamic.extended_dynamic_state3_color_write_mask != 0,
            host_visible_device_local: memory.memory_types[..memory.memory_type_count as usize]
                .iter()
                .any(|m| {
                    m.property_flags.contains(
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )
                }),
        })
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            if let Some((api, messenger)) = &self.debug {
                api.destroy_debug_utils_messenger(*messenger, None);
            }
            self.raw.destroy_instance(None);
        }
    }
}

pub struct Gpu {
    pub(crate) instance: Arc<Instance>,
    pub(crate) raw: ash::Device,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) allocator: Option<vk_mem::Allocator>,
    pub(crate) heap: ash::ext::descriptor_heap::Device,
    pub(crate) acceleration: ash::khr::acceleration_structure::Device,
    pub(crate) acceleration_properties:
        vk::PhysicalDeviceAccelerationStructurePropertiesKHR<'static>,
    pub(crate) dynamic_blend: Option<ash::ext::extended_dynamic_state3::Device>,
    pub(crate) debug_utils: Option<ash::ext::debug_utils::Device>,
    pub(crate) heap_properties: vk::PhysicalDeviceDescriptorHeapPropertiesEXT<'static>,
    pub(crate) limits: vk::PhysicalDeviceLimits,
    pub(crate) queue_family: u32,
    pub(crate) queues: Vec<Queue>,
    pub(crate) next_recording: std::sync::atomic::AtomicU64,
    pub(crate) pools: Mutex<Vec<(vk::CommandPool, vk::CommandBuffer)>>,
    pub(crate) pipelines: Mutex<super::pipeline_cache::PipelineCache>,
    pub info: AdapterInfo,
}

pub(crate) struct Queue {
    pub raw: vk::Queue,
    pub timeline: vk::Semaphore,
    pub submitted: Mutex<u64>,
}

impl Gpu {
    pub fn new(instance: Arc<Instance>, adapter_name: Option<&str>) -> Result<Arc<Self>> {
        Self::create(instance, adapter_name, None)
    }

    pub(crate) fn create(
        instance: Arc<Instance>,
        adapter_name: Option<&str>,
        surface: Option<vk::SurfaceKHR>,
    ) -> Result<Arc<Self>> {
        let physicals = unsafe { instance.raw.enumerate_physical_devices()? };
        let surface_api = ash::khr::surface::Instance::load(&instance.entry, &instance.raw);
        let mut rejected = Vec::new();
        for physical in physicals {
            let info = instance.adapter_info(physical)?;
            if adapter_name
                .is_some_and(|name| !info.name.to_lowercase().contains(&name.to_lowercase()))
            {
                continue;
            }
            if info.api_version < vk::API_VERSION_1_3
                || !info.descriptor_heap
                || !info.shader_untyped_pointers
                || !info.unified_image_layouts
                || !info.buffer_device_address
                || !info.timeline_semaphore
                || !info.dynamic_rendering
                || !info.synchronization2
            {
                rejected.push(format!("{}: requires Vulkan 1.3, descriptorHeap, shaderUntypedPointers, unifiedImageLayouts, bufferDeviceAddress, timelineSemaphore, dynamicRendering and synchronization2", info.name));
                continue;
            }
            let extensions = unsafe {
                instance
                    .raw
                    .enumerate_device_extension_properties(physical)?
            };
            if !info.acceleration_structure || !info.ray_query {
                rejected.push(format!(
                    "{}: directional shadows require accelerationStructure and rayQuery",
                    info.name
                ));
                continue;
            }
            let names = [
                ash::ext::descriptor_heap::NAME,
                ash::khr::shader_untyped_pointers::NAME,
                ash::khr::maintenance5::NAME,
                ash::khr::unified_image_layouts::NAME,
                ash::khr::acceleration_structure::NAME,
                ash::khr::ray_query::NAME,
                ash::khr::deferred_host_operations::NAME,
            ];
            if names.iter().any(|name| {
                !extensions
                    .iter()
                    .any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) == *name })
            }) {
                rejected.push(format!(
                    "{}: missing descriptor heap or ray query extension dependencies",
                    info.name
                ));
                continue;
            }
            let families = unsafe {
                instance
                    .raw
                    .get_physical_device_queue_family_properties(physical)
            };
            let family = families
                .iter()
                .enumerate()
                .find(|(index, family)| {
                    family
                        .queue_flags
                        .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                        && surface.is_none_or(|s| unsafe {
                            surface_api
                                .get_physical_device_surface_support(physical, *index as u32, s)
                                .unwrap_or(false)
                        })
                })
                .map(|(i, _)| i as u32);
            let Some(queue_family) = family else {
                rejected.push(format!("{}: no graphics/compute/present queue", info.name));
                continue;
            };
            let mut available11 = vk::PhysicalDeviceVulkan11Features::default();
            let mut available12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut available5 = vk::PhysicalDeviceMaintenance5FeaturesKHR::default();
            let mut available = vk::PhysicalDeviceFeatures2::default()
                .push(&mut available11)
                .push(&mut available12)
                .push(&mut available5);
            unsafe {
                instance
                    .raw
                    .get_physical_device_features2(physical, &mut available);
            }
            let core = available.features;
            if core.shader_int64 == 0
                || core.multi_draw_indirect == 0
                || core.draw_indirect_first_instance == 0
                || available11.shader_draw_parameters == 0
                || available12.scalar_block_layout == 0
                || available12.draw_indirect_count == 0
                || available5.maintenance5 == 0
            {
                rejected.push(format!(
                    "{}: missing shader integer, scalar layout, indirect or maintenance5 feature",
                    info.name
                ));
                continue;
            }
            let tensor = &info.cooperative;
            let tensor_enabled = tensor.memory_model && tensor.float16 && tensor.storage16;
            let vector_enabled = tensor_enabled && tensor.vector && tensor.replicated_composites;
            let matrix_enabled = tensor_enabled && tensor.matrix;
            let matrix2_enabled = matrix_enabled
                && tensor
                    .extensions
                    .iter()
                    .any(|s| s == "VK_NV_cooperative_matrix2");
            let mut cv = vk::PhysicalDeviceCooperativeVectorFeaturesNV::default()
                .cooperative_vector(vector_enabled);
            let mut replicated = vk::PhysicalDeviceShaderReplicatedCompositesFeaturesEXT::default()
                .shader_replicated_composites(vector_enabled);
            let mut cm = vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default()
                .cooperative_matrix(matrix_enabled);
            let mut cm2 = tensor.matrix2;
            let mut f11 = vk::PhysicalDeviceVulkan11Features::default()
                .shader_draw_parameters(true)
                .storage_buffer16_bit_access(tensor_enabled);
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
                .buffer_device_address(true)
                .timeline_semaphore(true)
                .scalar_block_layout(true)
                .draw_indirect_count(true);
            f12.shader_float16 = tensor_enabled.into();
            f12.vulkan_memory_model = tensor_enabled.into();
            f12.vulkan_memory_model_device_scope =
                (tensor_enabled && available12.vulkan_memory_model_device_scope != 0).into();
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
                .dynamic_rendering(true)
                .synchronization2(true);
            let mut f5 = vk::PhysicalDeviceMaintenance5FeaturesKHR::default().maintenance5(true);
            let mut fh =
                vk::PhysicalDeviceDescriptorHeapFeaturesEXT::default().descriptor_heap(true);
            let mut ft = vk::PhysicalDeviceShaderUntypedPointersFeaturesKHR::default()
                .shader_untyped_pointers(true);
            let mut fu = vk::PhysicalDeviceUnifiedImageLayoutsFeaturesKHR::default()
                .unified_image_layouts(true);
            let mut fb = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default()
                .extended_dynamic_state3_color_blend_enable(true)
                .extended_dynamic_state3_color_blend_equation(true)
                .extended_dynamic_state3_color_write_mask(true);
            let mut fa = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                .acceleration_structure(true);
            let mut fq = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
            let mut enabled: Vec<_> = names.iter().map(|name| name.as_ptr()).collect();
            if vector_enabled {
                enabled.push(ash::nv::cooperative_vector::NAME.as_ptr());
                enabled.push(ash::ext::shader_replicated_composites::NAME.as_ptr());
            }
            if matrix_enabled {
                enabled.push(ash::khr::cooperative_matrix::NAME.as_ptr());
            }
            if matrix2_enabled {
                enabled.push(ash::nv::cooperative_matrix2::NAME.as_ptr());
            }
            if info.dynamic_blend {
                enabled.push(ash::ext::extended_dynamic_state3::NAME.as_ptr());
            }
            if surface.is_some() {
                enabled.push(ash::khr::swapchain::NAME.as_ptr());
            }
            let priorities = vec![1.0; families[queue_family as usize].queue_count.min(2) as usize];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priorities)];
            let core = vk::PhysicalDeviceFeatures::default()
                .shader_int64(true)
                .multi_draw_indirect(true)
                .draw_indirect_first_instance(true)
                .image_cube_array(core.image_cube_array != 0)
                .texture_compression_bc(core.texture_compression_bc != 0);
            let mut create = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&enabled)
                .enabled_features(&core)
                .push(&mut f11)
                .push(&mut f12)
                .push(&mut f13)
                .push(&mut f5)
                .push(&mut fh)
                .push(&mut ft)
                .push(&mut fu)
                .push(&mut fa)
                .push(&mut fq);
            if vector_enabled {
                create = create.push(&mut cv).push(&mut replicated);
            }
            if matrix_enabled {
                create = create.push(&mut cm);
            }
            if matrix2_enabled {
                create = create.push(&mut cm2);
            }
            if info.dynamic_blend {
                create = create.push(&mut fb);
            }
            let raw = unsafe { instance.raw.create_device(physical, &create, None)? };
            let mut allocator_info =
                vk_mem::AllocatorCreateInfo::new(&instance.raw, &raw, physical);
            allocator_info.vulkan_api_version = vk::API_VERSION_1_3;
            allocator_info.flags = vk_mem::AllocatorCreateFlags::BUFFER_DEVICE_ADDRESS;
            let allocator = match unsafe { vk_mem::Allocator::new(allocator_info) } {
                Ok(allocator) => allocator,
                Err(error) => {
                    unsafe {
                        raw.destroy_device(None);
                    }
                    return Err(error.into());
                }
            };
            let mut queues: Vec<Queue> = Vec::new();
            for index in 0..priorities.len() {
                let mut semaphore_type = vk::SemaphoreTypeCreateInfo::default()
                    .semaphore_type(vk::SemaphoreType::TIMELINE);
                let timeline = match unsafe {
                    raw.create_semaphore(
                        &vk::SemaphoreCreateInfo::default().push(&mut semaphore_type),
                        None,
                    )
                } {
                    Ok(timeline) => timeline,
                    Err(error) => {
                        for queue in &queues {
                            unsafe {
                                raw.destroy_semaphore(queue.timeline, None);
                            }
                        }
                        drop(allocator);
                        unsafe {
                            raw.destroy_device(None);
                        }
                        return Err(error.into());
                    }
                };
                queues.push(Queue {
                    raw: unsafe { raw.get_device_queue(queue_family, index as u32) },
                    timeline,
                    submitted: Mutex::new(0),
                });
            }
            let mut heap_properties = vk::PhysicalDeviceDescriptorHeapPropertiesEXT::default();
            let mut acceleration_properties =
                vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
            let mut properties = vk::PhysicalDeviceProperties2::default()
                .push(&mut heap_properties)
                .push(&mut acceleration_properties);
            unsafe {
                instance
                    .raw
                    .get_physical_device_properties2(physical, &mut properties);
            }
            let limits = properties.properties.limits;
            let heap = ash::ext::descriptor_heap::Device::load(&instance.raw, &raw);
            let acceleration = ash::khr::acceleration_structure::Device::load(&instance.raw, &raw);
            heap_properties.p_next = std::ptr::null_mut();
            acceleration_properties.p_next = std::ptr::null_mut();
            let dynamic_blend = info
                .dynamic_blend
                .then(|| ash::ext::extended_dynamic_state3::Device::load(&instance.raw, &raw));
            let debug_utils = instance
                .validation_enabled()
                .then(|| ash::ext::debug_utils::Device::load(&instance.raw, &raw));
            return Ok(Arc::new(Self {
                instance,
                raw,
                physical,
                allocator: Some(allocator),
                heap,
                acceleration,
                acceleration_properties,
                dynamic_blend,
                debug_utils,
                heap_properties,
                limits,
                queue_family,
                queues,
                next_recording: std::sync::atomic::AtomicU64::new(0),
                pools: Mutex::new(Vec::new()),
                pipelines: Mutex::new(Default::default()),
                info,
            }));
        }
        anyhow::bail!("No compatible adapter. {}", rejected.join("; "))
    }

    pub(crate) fn allocator(&self) -> &vk_mem::Allocator {
        self.allocator.as_ref().unwrap()
    }

    pub fn completed_value(&self) -> Result<u64> {
        self.completed_on(0)
    }

    pub fn queue_count(&self) -> usize {
        self.queues.len()
    }

    pub fn completed_on(&self, queue: usize) -> Result<u64> {
        let queue = self.queues.get(queue).context("queue is unavailable")?;
        Ok(unsafe { self.raw.get_semaphore_counter_value(queue.timeline)? })
    }

    pub fn wait(&self, value: u64, timeout_ns: u64) -> Result<()> {
        self.wait_on(0, value, timeout_ns)
    }

    pub fn wait_on(&self, queue: usize, value: u64, timeout_ns: u64) -> Result<()> {
        let queue = self.queues.get(queue).context("queue is unavailable")?;
        let semaphores = [queue.timeline];
        let values = [value];
        unsafe {
            self.raw.wait_semaphores(
                &vk::SemaphoreWaitInfo::default()
                    .semaphores(&semaphores)
                    .values(&values),
                timeout_ns,
            )?;
        }
        Ok(())
    }

    pub fn wait_idle(&self) -> Result<()> {
        let _queues: Vec<_> = self
            .queues
            .iter()
            .map(|q| q.submitted.lock().unwrap_or_else(|p| p.into_inner()))
            .collect();
        unsafe {
            self.raw.device_wait_idle()?;
        }
        Ok(())
    }

    pub fn validation_errors(&self) -> Vec<String> {
        self.instance.validation_errors()
    }
    pub fn instance(&self) -> &Arc<Instance> {
        &self.instance
    }
    pub fn pipeline_count(&self) -> usize {
        self.pipelines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
    pub fn try_pipeline_count(&self) -> Option<usize> {
        self.pipelines.try_lock().ok().map(|cache| cache.len())
    }
    pub fn allocation_count(&self) -> Result<u32> {
        Ok(self
            .allocator()
            .calculate_statistics()?
            .total
            .statistics
            .allocationCount)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            let _ = self.raw.device_wait_idle();
            for (pool, _) in self
                .pools
                .get_mut()
                .unwrap_or_else(|p| p.into_inner())
                .drain(..)
            {
                self.raw.destroy_command_pool(pool, None);
            }
            for queue in &self.queues {
                self.raw.destroy_semaphore(queue.timeline, None);
            }
            drop(self.allocator.take());
            self.raw.destroy_device(None);
        }
    }
}
