use zenith_core::log;
use zenith_rhi::*;
#[path = "minimal_tests/direct_heaps.rs"]
mod direct_heaps;
#[path = "minimal_tests/draw_arguments.rs"]
mod draw_arguments;
#[path = "minimal_tests/images.rs"]
mod images;
#[path = "minimal_tests/indirect.rs"]
mod indirect;
#[path = "minimal_tests/lifetime.rs"]
mod lifetime;
#[path = "minimal_tests/raster_states.rs"]
mod raster_states;
#[path = "minimal_tests/shader_reload.rs"]
mod shader_reload;

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Root {
    input: u64,
    output: u64,
    count: u32,
    multiplier: u32,
}

fn main() -> anyhow::Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "acceptance tests require VK_LAYER_KHRONOS_validation; configure VK_LAYER_PATH"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    anyhow::ensure!(
        gpu.compile_shader(
            "zenith-rhi/tests/shaders/transform.slang",
            "transform",
            ShaderStage::Vertex
        )
        .is_err(),
        "incorrect shader stage accepted"
    );
    log::info!(
        "{}; validation={}",
        gpu.info.name,
        instance.validation_enabled()
    );
    let count = 1027;
    let data: Vec<u32> = (0..count).map(|i| i * 13 + 7).collect();
    let upload = gpu.allocate((count * 4) as u64, MemoryDomain::Upload)?;
    upload.write(0, bytemuck::cast_slice(&data))?;
    log::info!("Upload memory: {:?}", upload.host_memory_properties());
    let input = gpu.allocate(upload.size(), MemoryDomain::Device)?;
    let output = gpu.allocate(upload.size(), MemoryDomain::Device)?;
    let readback = gpu.allocate(upload.size(), MemoryDomain::Readback)?;
    let mut args = Arguments::new(&gpu, 1024)?;
    let root = args.push(&Root {
        input: input.address().value(),
        output: output.address().value(),
        count,
        multiplier: 19,
    })?;
    let shader = gpu.compile_shader(
        "zenith-rhi/tests/shaders/transform.slang",
        "transform",
        ShaderStage::Compute,
    )?;
    let pipeline = gpu.compute(&shader)?;
    let mut commands = gpu.commands()?;
    commands.copy(&upload.whole(), &input.whole())?;
    anyhow::ensure!(
        upload.write(0, &[0; 4]).is_err(),
        "live GPU memory accepted a CPU write"
    );
    commands.barrier(Access::COPY_WRITE, Access::COMPUTE_READ)?;
    unsafe {
        commands.dispatch(
            &pipeline,
            &root,
            [count.div_ceil(64), 1, 1],
            &[input.whole(), output.whole()],
        )?;
    }
    commands.barrier(Access::COMPUTE_WRITE, Access::COPY_READ)?;
    commands.copy(&output.whole(), &readback.whole())?;
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    let mut submission = commands.submit()?;
    submission.wait(10_000_000_000)?;
    let mut result = vec![0u32; count as usize];
    readback.read(0, bytemuck::cast_slice_mut(&mut result))?;
    for (i, (&actual, &value)) in result.iter().zip(data.iter()).enumerate() {
        anyhow::ensure!(
            actual == value * 19 + i as u32,
            "compute mismatch at {i}: {actual}"
        );
    }
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "Vulkan validation failed"
    );
    log::info!(
        "PASS: VMA upload/copy, address-based compute (1027 values), readback, completion and CPU write exclusion"
    );
    image_test(&gpu)?;
    direct_heaps::run(&gpu)?;
    raster_test(&gpu)?;
    indirect::run(&gpu)?;
    images::run(&gpu)?;
    raster_states::run(&gpu)?;
    draw_arguments::run(&gpu)?;
    shader_reload::run(&gpu)?;
    lifetime::run(&gpu)?;
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "Vulkan validation failed"
    );
    Ok(())
}

fn raster_test(gpu: &std::sync::Arc<Gpu>) -> anyhow::Result<()> {
    use ash::vk;
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct Vertex {
        position: [f32; 2],
        padding: [f32; 2],
        color: [f32; 4],
    }
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct RasterRoot {
        vertices: u64,
        tint: [f32; 4],
    }
    let vertices = [
        Vertex {
            position: [-1.0, -1.0],
            padding: [0.0; 2],
            color: [1.0, 0.0, 0.0, 1.0],
        },
        Vertex {
            position: [3.0, -1.0],
            padding: [0.0; 2],
            color: [1.0, 0.0, 0.0, 1.0],
        },
        Vertex {
            position: [-1.0, 3.0],
            padding: [0.0; 2],
            color: [1.0, 0.0, 0.0, 1.0],
        },
    ];
    let data = gpu.allocate(
        std::mem::size_of_val(&vertices) as u64,
        MemoryDomain::Upload,
    )?;
    data.write(0, bytemuck::cast_slice(&vertices))?;
    let indices = gpu.allocate(8, MemoryDomain::Upload)?;
    indices.write(0, bytemuck::cast_slice(&[0u16, 1, 2, 0]))?;
    let mut arguments = Arguments::new(gpu, 128)?;
    let root = arguments.push(&RasterRoot {
        vertices: data.address().value(),
        tint: [0.5, 1.0, 1.0, 1.0],
    })?;
    let image = gpu.texture(TextureDesc::color(8, 8, vk::Format::R8G8B8A8_UNORM))?;
    let view = image.full_view()?;
    let readback = gpu.allocate(8 * 8 * 4, MemoryDomain::Readback)?;
    let vertex = gpu.compile_shader(
        "zenith-rhi/tests/shaders/raster.slang",
        "vertexMain",
        ShaderStage::Vertex,
    )?;
    let fragment = gpu.compile_shader(
        "zenith-rhi/tests/shaders/raster.slang",
        "fragmentMain",
        ShaderStage::Fragment,
    )?;
    let pipeline = gpu.raster(&RasterDesc {
        vertex: &vertex,
        fragment: &fragment,
        colors: &[vk::Format::R8G8B8A8_UNORM],
        depth: vk::Format::UNDEFINED,
        stencil: vk::Format::UNDEFINED,
        samples: vk::SampleCountFlags::TYPE_1,
        topology: vk::PrimitiveTopology::TRIANGLE_LIST,
        dynamic_blend: false,
        blend: &[Blend::default()],
    })?;
    let mut commands = gpu.commands()?;
    unsafe {
        commands.initialize(&image)?;
    }
    commands.begin_rendering(
        &[Attachment {
            view: &view,
            clear: Some([0.0, 0.0, 0.0, 1.0]),
            store: true,
            resolve: None,
        }],
        None,
        vk::Extent2D {
            width: 8,
            height: 8,
        },
    )?;
    unsafe {
        commands.draw_indexed(
            &pipeline,
            &root,
            &root,
            &indices.slice(0..6)?,
            vk::IndexType::UINT16,
            0,
            0..1,
            &[data.whole()],
        )?;
    }
    commands.end_rendering()?;
    commands.barrier(
        Access {
            stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        },
        Access::COPY_READ,
    )?;
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: 8,
            height: 8,
            depth: 1,
        });
    unsafe {
        commands.read_image(&image, &readback.whole(), &[region])?;
    }
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut pixels = [0u8; 256];
    readback.read(0, &mut pixels)?;
    for pixel in pixels.chunks_exact(4) {
        anyhow::ensure!(
            (127..=128).contains(&pixel[0]) && pixel[1..] == [0, 0, 255],
            "raster readback mismatch: {pixel:?}"
        );
    }
    log::info!(
        "PASS: vertex pulling, fragment root address, 16-bit indexed raster, attachment and pixel readback (1 UNORM step tolerance)"
    );
    Ok(())
}

fn image_test(gpu: &std::sync::Arc<Gpu>) -> anyhow::Result<()> {
    use ash::vk;
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct ImageRoot {
        image: u32,
        sampler: u32,
        output: u32,
        padding: u32,
        result: u64,
    }
    let table = Descriptors::new(gpu, 16, 4)?;
    let texture = gpu.texture(TextureDesc::color(3, 5, vk::Format::R32G32B32A32_SFLOAT))?;
    let mut storage_desc = TextureDesc::color(3, 5, vk::Format::R32G32B32A32_SFLOAT);
    storage_desc.usage |= vk::ImageUsageFlags::STORAGE;
    let storage = gpu.texture(storage_desc)?;
    let image = table.image(&texture.full_view()?, false)?;
    let output = table.image(&storage.full_view()?, true)?;
    let sampler = table.sampler(vk::Filter::NEAREST, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
    let mut commands = gpu.commands()?;
    let copied = table.copy_image(&mut commands, &image)?;
    let result = gpu.allocate(16, MemoryDomain::Readback)?;
    let image_result = gpu.allocate(16, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(gpu, 64)?;
    let root = arguments.push(&ImageRoot {
        image: copied.index(),
        sampler: sampler.index(),
        output: output.index(),
        padding: 0,
        result: result.address().value(),
    })?;
    let shader = gpu.compile_shader(
        "zenith-rhi/tests/shaders/images.slang",
        "sampleImage",
        ShaderStage::Compute,
    )?;
    let pipeline = gpu.compute(&shader)?;
    commands.bind_descriptors(&table)?;
    commands.retain_image(&image)?;
    commands.retain_image(&output)?;
    commands.retain_sampler(&sampler)?;
    unsafe {
        commands.initialize(&texture)?;
        commands.initialize(&storage)?;
    }
    commands.clear_color(&texture, [0.25, 0.5, 0.75, 1.0])?;
    commands.barrier(Access::COPY_WRITE, Access::COMPUTE_READ)?;
    commands.barrier(
        Access::COPY_WRITE,
        Access {
            stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
            access: vk::AccessFlags2::RESOURCE_HEAP_READ_EXT,
        },
    )?;
    unsafe {
        commands.dispatch(&pipeline, &root, [1, 1, 1], &[result.whole()])?;
    }
    commands.barrier(Access::COMPUTE_WRITE, Access::COPY_READ)?;
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: 1,
            height: 1,
            depth: 1,
        });
    unsafe {
        commands.read_image(&storage, &image_result.whole(), &[region])?;
    }
    commands.barrier(Access::ALL, Access::HOST_READ)?;
    drop(image);
    drop(output);
    drop(sampler);
    drop(copied);
    anyhow::ensure!(
        table.available_images() == 13,
        "in-flight descriptor reused"
    );
    let mut submission = commands.submit()?;
    submission.wait(10_000_000_000)?;
    anyhow::ensure!(
        table.available_images() == 16,
        "completed descriptors not retired"
    );
    let array = table.image_array(&vec![texture.full_view()?; 16], false)?;
    anyhow::ensure!(
        array.windows(2).all(|a| a[1].index() == a[0].index() + 1),
        "noncontiguous image array"
    );
    anyhow::ensure!(
        table.image(&texture.full_view()?, false).is_err(),
        "heap exhaustion accepted"
    );
    drop(array);
    for memory in [&result, &image_result] {
        let mut actual = [0.0f32; 4];
        memory.read(0, bytemuck::cast_slice_mut(&mut actual))?;
        anyhow::ensure!(
            actual == [0.25, 0.5, 0.75, 1.0],
            "descriptor sampling/storage mismatch: {actual:?}"
        );
    }
    log::info!(
        "PASS: descriptor heap sampling, GPU descriptor copy, storage-image readback, contiguous arrays/exhaustion and retired slots"
    );
    Ok(())
}
