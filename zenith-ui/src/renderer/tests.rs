use super::*;
use egui::{
    epaint::{ImageDelta, Mesh, Vertex},
    pos2, Color32, ColorImage, Rect,
};
use zenith_rendergraph::ResourceCache;

#[test]
fn clipping_scales_and_clamps_to_the_attachment() {
    let extent = vk::Extent2D {
        width: 100,
        height: 80,
    };
    let clipped = clip_rect(
        Rect::from_min_max(pos2(-4.0, 3.0), pos2(80.0, 30.0)),
        2.0,
        extent,
    )
    .unwrap();
    assert_eq!(clipped.offset, vk::Offset2D { x: 0, y: 6 });
    assert_eq!(
        clipped.extent,
        vk::Extent2D {
            width: 100,
            height: 54
        }
    );
    assert!(clip_rect(
        Rect::from_min_max(pos2(60.0, 0.0), pos2(80.0, 20.0)),
        2.0,
        extent
    )
    .is_none());
    assert!(clip_rect(Rect::NOTHING, 1.0, extent).is_none());
    assert!(clip_rect(
        Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(10.0, 10.0)),
        1.0,
        extent
    )
    .is_none());
    assert_eq!(
        clip_rect(Rect::EVERYTHING, 1.0, extent).unwrap().extent,
        extent
    );
}

fn quad(id: TextureId, rect: Rect, clip: Rect, color: Color32) -> ClippedPrimitive {
    let mut mesh = Mesh::with_texture(id);
    mesh.vertices = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ]
    .map(|pos| Vertex {
        pos,
        uv: pos2(0.75, 0.75),
        color,
    })
    .to_vec();
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    ClippedPrimitive {
        clip_rect: clip,
        primitive: Primitive::Mesh(mesh),
    }
}

fn enqueue(
    renderer: &mut UiRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    cache: &mut ResourceCache,
    format: vk::Format,
    primitives: Vec<ClippedPrimitive>,
    textures: TexturesDelta,
) -> Result<(Submission, Arc<Memory>)> {
    let mut graph = RenderGraphBuilder::new(gpu, descriptors, cache)?;
    let target = graph.create_image(TextureDesc::color(16, 16, format))?;
    graph.pass(
        "background",
        vec![target.write(Access::COPY_WRITE)],
        move |ctx| {
            ctx.commands
                .clear_color(&ctx.image(target)?, [0.0, 0.0, 0.0, 1.0])
        },
    )?;
    renderer.paint(&mut graph, target, 2.0, primitives, textures)?;
    let readback = gpu.allocate(16 * 16 * 4, MemoryDomain::Readback)?;
    let destination = graph.import_buffer(readback.clone());
    graph.pass(
        "ui_readback",
        vec![
            target.read(Access::COPY_READ),
            destination.write(Access::COPY_WRITE),
        ],
        move |ctx| {
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: 16,
                    height: 16,
                    depth: 1,
                });
            unsafe {
                ctx.commands
                    .read_image(&ctx.image(target)?, &ctx.buffer(destination)?, &[region])
            }
        },
    )?;
    Ok((graph.record()?.submit()?, readback))
}

#[test]
#[ignore = "requires Vulkan validation, Slang, and a supported GPU"]
fn textures_clipping_blending_and_in_flight_retirement() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "UI test requires Vulkan validation"
    );
    let gpu = Gpu::new(
        instance.clone(),
        std::env::var("ZENITH_ADAPTER").ok().as_deref(),
    )?;
    {
        let descriptors = Descriptors::new(&gpu, 64, 16)?;
        let id = TextureId::Managed(1);
        let second = TextureId::Managed(2);
        let full_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 8.0));
        let clip = Rect::from_min_max(pos2(1.0, 1.0), pos2(5.0, 5.0));
        for format in [vk::Format::R8G8B8A8_UNORM, vk::Format::R8G8B8A8_SRGB] {
            let mut renderer = UiRenderer::new(&gpu)?;
            let mut caches: [ResourceCache; 3] = std::array::from_fn(|_| ResourceCache::default());
            let mut pending = Vec::new();
            for frame in 0..3 {
                let delta = match frame {
                    0 => ImageDelta::full(
                        ColorImage::new([2, 2], vec![Color32::WHITE; 4]),
                        TextureOptions::NEAREST,
                    ),
                    1 => ImageDelta::partial(
                        [1, 1],
                        ColorImage::new([1, 1], vec![Color32::GREEN]),
                        TextureOptions::LINEAR,
                    ),
                    _ => ImageDelta::full(
                        ColorImage::new([1, 1], vec![Color32::BLUE]),
                        TextureOptions::NEAREST,
                    ),
                };
                let mut textures = TexturesDelta::default();
                textures.set.entry(id).or_default().push(delta);
                if frame == 0 {
                    textures
                        .set
                        .entry(second)
                        .or_default()
                        .push(ImageDelta::full(
                            ColorImage::new([1, 1], vec![Color32::WHITE]),
                            TextureOptions::NEAREST,
                        ));
                }
                if frame == 2 {
                    textures.free.extend([id, second]);
                }
                let color = if frame == 0 {
                    Color32::from_rgb(128, 64, 32)
                } else {
                    Color32::WHITE
                };
                let primitives = vec![
                    quad(id, full_rect, clip, color),
                    quad(
                        second,
                        Rect::from_min_max(pos2(6.0, 1.0), pos2(8.0, 3.0)),
                        full_rect,
                        Color32::from_rgba_premultiplied(128, 0, 0, 128),
                    ),
                    quad(
                        second,
                        Rect::from_min_max(pos2(6.0, 1.0), pos2(8.0, 3.0)),
                        full_rect,
                        Color32::from_rgba_premultiplied(0, 64, 0, 128),
                    ),
                    quad(
                        second,
                        Rect::from_min_max(pos2(6.0, 4.0), pos2(8.0, 6.0)),
                        full_rect,
                        Color32::WHITE,
                    ),
                    quad(
                        second,
                        Rect::from_min_max(pos2(6.0, 4.0), pos2(8.0, 6.0)),
                        full_rect,
                        Color32::from_black_alpha(128),
                    ),
                    quad(
                        second,
                        Rect::from_min_max(pos2(6.0, 6.0), pos2(8.0, 8.0)),
                        full_rect,
                        Color32::from_rgb_additive(64, 0, 0),
                    ),
                ];
                pending.push(enqueue(
                    &mut renderer,
                    &gpu,
                    &descriptors,
                    &mut caches[frame],
                    format,
                    primitives,
                    textures,
                )?);
            }
            assert!(renderer.textures.is_empty());
            drop(renderer);
            for (frame, (mut submission, readback)) in pending.into_iter().enumerate() {
                submission.wait(10_000_000_000)?;
                let mut pixels = vec![0; 16 * 16 * 4];
                readback.read(0, &mut pixels)?;
                let pixel = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..(y * 16 + x + 1) * 4];
                let expected = [[128, 64, 32, 255], [0, 255, 0, 255], [0, 0, 255, 255]][frame];
                for y in 0..16 {
                    for x in 0..12 {
                        let expected = if (2..10).contains(&x) && (2..10).contains(&y) {
                            expected
                        } else {
                            [0, 0, 0, 255]
                        };
                        assert!(
                            pixel(x, y)
                                .iter()
                                .zip(expected)
                                .all(|(a, b)| a.abs_diff(b) <= 1),
                            "{format:?} frame {frame}, ({x}, {y}): {:?} != {expected:?}",
                            pixel(x, y)
                        );
                    }
                }
                let blended = if format == vk::Format::R8G8B8A8_SRGB {
                    [74, 74, 0, 255]
                } else {
                    [64, 64, 0, 255]
                };
                assert!(
                    pixel(14, 3)
                        .iter()
                        .zip(blended)
                        .all(|(a, b)| a.abs_diff(b) <= 2),
                    "blending {format:?}: {:?}",
                    pixel(14, 3)
                );
                assert!(
                    pixel(14, 9)
                        .iter()
                        .zip([127, 127, 127, 255])
                        .all(|(a, b)| a.abs_diff(b) <= 1),
                    "gamma blending {format:?}: {:?}",
                    pixel(14, 9)
                );
                assert!(
                    pixel(14, 13)
                        .iter()
                        .zip([64, 0, 0, 255])
                        .all(|(a, b)| a.abs_diff(b) <= 1),
                    "additive blending {format:?}: {:?}",
                    pixel(14, 13)
                );
            }
        }
    }
    gpu.wait_idle()?;
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "{:?}",
        instance.validation_errors()
    );
    Ok(())
}
