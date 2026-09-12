use anyhow::{ensure, Result};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use zenith_core::log;
use zenith_rendergraph::*;
use zenith_rhi::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Root {
    input: GpuAddress,
    output: GpuAddress,
    count: u32,
    multiplier: u32,
}

fn main() -> Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "graph acceptance requires VK_LAYER_KHRONOS_validation; set VK_LAYER_PATH to your Vulkan SDK's Bin directory (this workspace: target/vulkan-sdk/Bin), or run scripts/validate-vulkan.ps1 -GraphOnly"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let table = Descriptors::new(&gpu, 64, 8)?;
        buffers(&gpu, &table)?;
        images(&gpu, &table)?;
        handles(&gpu, &table)?;
    }
    gpu.wait_idle()?;
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "validation errors: {:?}",
        instance.validation_errors()
    );
    log::info!("PASS: graph RAW/WAR/WAW, distinct reader stages, byte/subresource ranges, transient retirement, exports, foreign and undeclared handles");
    Ok(())
}

fn buffers(gpu: &Arc<Gpu>, table: &Arc<Descriptors>) -> Result<()> {
    let shader = gpu.compile_shader(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zenith-rhi/tests/shaders/transform.slang"
        ),
        "transform",
        ShaderStage::Compute,
    )?;
    let pipeline = gpu.compute(&shader)?;
    let mut cache = ResourceCache::default();
    let readback = gpu.allocate(256, MemoryDomain::Readback)?;
    let mut allocations = None;
    for _ in 0..32 {
        let mut graph = RenderGraphBuilder::new(gpu, table, &mut cache)?;
        let buffer = graph.create_buffer(256, MemoryDomain::Device)?;
        let second = graph.create_buffer(128, MemoryDomain::Device)?;
        let output = graph.import_buffer(readback.clone());
        let alias = graph.import_buffer(graph.export_buffer(buffer)?);
        ensure!(
            alias == buffer,
            "imported aliases have different identities"
        );
        for (range, value) in [(0..128, 5), (128..256, 7), (0..128, 9)] {
            graph.pass(
                "fill",
                vec![buffer.write(Access::COPY_WRITE).bytes(range.clone())],
                move |ctx| {
                    let destination = ctx.buffer_range(buffer, range)?;
                    ctx.commands.fill(&destination, value)
                },
            )?;
        }
        let compute = pipeline.clone();
        graph.pass(
            "compute",
            vec![
                buffer.read(Access::COMPUTE_READ).bytes(0..128),
                buffer.write(Access::COMPUTE_WRITE).bytes(128..256),
            ],
            move |ctx| {
                let input = ctx.buffer_range(buffer, 0..128)?;
                let output = ctx.buffer_range(buffer, 128..256)?;
                let root = ctx.arguments(&Root {
                    input: input.address(),
                    output: output.address(),
                    count: 32,
                    multiplier: 2,
                })?;
                unsafe {
                    ctx.commands
                        .dispatch(&compute, &root, [1, 1, 1], &[input, output])
                }
            },
        )?;
        graph.pass(
            "copy first reader",
            vec![
                buffer.read(Access::COPY_READ).bytes(128..256),
                output.write(Access::COPY_WRITE).bytes(0..128),
            ],
            move |ctx| {
                ctx.commands.copy(
                    &ctx.buffer_range(buffer, 128..256)?,
                    &ctx.buffer_range(output, 0..128)?,
                )
            },
        )?;
        let compute = pipeline.clone();
        graph.pass(
            "compute second reader",
            vec![
                buffer.read(Access::COMPUTE_READ).bytes(128..256),
                second.write(Access::COMPUTE_WRITE),
            ],
            move |ctx| {
                let input = ctx.buffer_range(buffer, 128..256)?;
                let output = ctx.buffer(second)?;
                let root = ctx.arguments(&Root {
                    input: input.address(),
                    output: output.address(),
                    count: 32,
                    multiplier: 3,
                })?;
                unsafe {
                    ctx.commands
                        .dispatch(&compute, &root, [1, 1, 1], &[input, output])
                }
            },
        )?;
        graph.pass(
            "readback",
            vec![
                second.read(Access::COPY_READ),
                output.write(Access::COPY_WRITE).bytes(128..256),
            ],
            move |ctx| {
                ctx.commands
                    .copy(&ctx.buffer(second)?, &ctx.buffer_range(output, 128..256)?)
            },
        )?;
        graph.pass("host", vec![output.read(Access::HOST_READ)], |_| Ok(()))?;
        graph.record()?.submit()?.wait(10_000_000_000)?;
        let mut bytes = [0u8; 256];
        readback.read(0, &mut bytes)?;
        let values: &[u32] = bytemuck::cast_slice(&bytes);
        for i in 0..32 {
            ensure!(
                values[i] == 18 + i as u32 && values[32 + i] == 54 + 4 * i as u32,
                "graph transform mismatch"
            );
        }
        let count = gpu.allocation_count()?;
        ensure!(
            allocations.is_none_or(|previous| previous == count),
            "graph allocation count grew after completion"
        );
        allocations = Some(count);
    }
    let mut graph = RenderGraphBuilder::new(gpu, table, &mut cache)?;
    let data = graph.create_buffer(128, MemoryDomain::Device)?;
    let exported = graph.export_buffer(data)?;
    graph.pass("retain", vec![data.write(Access::COPY_WRITE)], move |ctx| {
        ctx.commands.fill(&ctx.buffer(data)?, 1)
    })?;
    let commands = graph.record()?;
    let mut graph = RenderGraphBuilder::new(gpu, table, &mut cache)?;
    let other = graph.create_buffer(128, MemoryDomain::Device)?;
    ensure!(
        !Arc::ptr_eq(&exported, &graph.export_buffer(other)?),
        "live transient was reused"
    );
    drop(graph);
    drop(commands);
    Ok(())
}

fn images(gpu: &Arc<Gpu>, table: &Arc<Descriptors>) -> Result<()> {
    let mut cache = ResourceCache::default();
    let readback = gpu.allocate(160, MemoryDomain::Readback)?;
    let mut graph = RenderGraphBuilder::new(gpu, table, &mut cache)?;
    let mut desc = TextureDesc::color(4, 4, vk::Format::R8G8B8A8_UNORM);
    desc.layers = 2;
    desc.mip_levels = 2;
    let image = graph.create_image(desc)?;
    for mip in 0..2 {
        for layer in 0..2 {
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(mip)
                .level_count(1)
                .base_array_layer(layer)
                .layer_count(1);
            graph.pass(
                "subresource clear",
                vec![image
                    .write(Access {
                        stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                        access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    })
                    .subresources(range)],
                move |ctx| {
                    let view = ctx.view_range(image, vk::ImageViewType::TYPE_2D, range)?;
                    ensure!(ctx.view(image).is_err(), "undeclared full view accepted");
                    ctx.commands.begin_rendering(
                        &[Attachment {
                            view: &view,
                            clear: Some([mip as f32, layer as f32, 0.0, 1.0]),
                            store: true,
                            resolve: None,
                        }],
                        None,
                        vk::Extent2D {
                            width: 4 >> mip,
                            height: 4 >> mip,
                        },
                    )?;
                    ctx.commands.end_rendering()
                },
            )?;
        }
    }
    let output = graph.import_buffer(readback.clone());
    graph.pass(
        "image readback",
        vec![
            image.read(Access::COPY_READ),
            output.write(Access::COPY_WRITE),
        ],
        move |ctx| {
            let mut regions = Vec::new();
            let mut offset = 0;
            for mip in 0..2 {
                let extent = vk::Extent3D {
                    width: 4 >> mip,
                    height: 4 >> mip,
                    depth: 1,
                };
                regions.push(
                    vk::BufferImageCopy::default()
                        .buffer_offset(offset)
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .mip_level(mip)
                                .layer_count(2),
                        )
                        .image_extent(extent),
                );
                offset += extent.width as u64 * extent.height as u64 * 8;
            }
            unsafe {
                ctx.commands
                    .read_image(&ctx.image(image)?, &ctx.buffer(output)?, &regions)
            }
        },
    )?;
    graph.pass("host", vec![output.read(Access::HOST_READ)], |_| Ok(()))?;
    graph.record()?.submit()?.wait(10_000_000_000)?;
    let mut bytes = [0u8; 160];
    readback.read(0, &mut bytes)?;
    let mut offset = 0;
    for mip in 0..2 {
        for layer in 0..2 {
            for _ in 0..(4 >> mip) * (4 >> mip) {
                ensure!(
                    bytes[offset..offset + 4] == [mip * 255, layer * 255, 0, 255],
                    "graph subresource clear mismatch"
                );
                offset += 4;
            }
        }
    }
    Ok(())
}

fn handles(gpu: &Arc<Gpu>, table: &Arc<Descriptors>) -> Result<()> {
    let mut a_cache = ResourceCache::default();
    let mut b_cache = ResourceCache::default();
    let mut a = RenderGraphBuilder::new(gpu, table, &mut a_cache)?;
    let mut b = RenderGraphBuilder::new(gpu, table, &mut b_cache)?;
    let a_buffer = a.create_buffer(64, MemoryDomain::Device)?;
    let b_buffer = b.create_buffer(64, MemoryDomain::Device)?;
    ensure!(
        a.pass(
            "foreign",
            vec![b_buffer.read(Access::COPY_READ)],
            |_| Ok(())
        )
        .is_err(),
        "foreign handle accepted"
    );
    ensure!(
        a.pass(
            "bounds",
            vec![a_buffer.read(Access::COPY_READ).bytes(0..65)],
            |_| Ok(())
        )
        .is_err(),
        "invalid range accepted"
    );
    a.pass(
        "uninitialized",
        vec![a_buffer.read(Access::COPY_READ)],
        |_| Ok(()),
    )?;
    ensure!(a.record().is_err(), "uninitialized read accepted");
    b.pass("undeclared", Vec::new(), move |ctx| {
        ensure!(ctx.buffer(b_buffer).is_err(), "undeclared handle accepted");
        Ok(())
    })?;
    drop(b.record()?);
    Ok(())
}
