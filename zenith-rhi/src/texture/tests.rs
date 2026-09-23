use super::*;
use crate::{Attachment, Instance};

#[test]
#[ignore = "requires Vulkan validation and a supported GPU"]
fn cached_full_views_share_the_handle_and_keep_the_texture_alive() -> Result<()> {
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "Vulkan validation is required"
    );
    let gpu = Gpu::new(instance.clone(), None)?;
    {
        let texture = gpu.texture(TextureDesc::color(8, 8, vk::Format::R8G8B8A8_UNORM))?;
        let weak = Arc::downgrade(&texture);
        let barrier = std::sync::Barrier::new(8);
        let views = std::thread::scope(|threads| {
            let barrier = &barrier;
            let jobs: Vec<_> = (0..8)
                .map(|_| {
                    let texture = &texture;
                    threads.spawn(move || {
                        barrier.wait();
                        texture.full_view().unwrap()
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        let raw = views[0].raw;
        assert!(views.iter().all(|view| view.raw == raw));
        drop(views);
        let view = texture.full_view()?;
        assert_eq!(view.raw, raw);
        let separate = texture.view(vk::ImageViewType::TYPE_2D, texture.desc().range())?;
        assert_ne!(separate.raw, raw);
        drop(separate);
        drop(texture);
        assert!(weak.upgrade().is_some());
        let mut commands = gpu.commands()?;
        unsafe {
            commands.initialize(view.texture())?;
        }
        commands.begin_rendering(
            &[Attachment {
                view: &view,
                clear: Some([1.0, 0.0, 0.0, 1.0]),
                store: true,
                resolve: None,
            }],
            None,
            vk::Extent2D {
                width: 8,
                height: 8,
            },
        )?;
        commands.end_rendering()?;
        drop(view);
        assert!(weak.upgrade().is_some());
        commands.submit()?.wait(10_000_000_000)?;
        assert!(weak.upgrade().is_none());
    }
    drop(gpu);
    ensure!(
        instance.validation_errors().is_empty(),
        "{:?}",
        instance.validation_errors()
    );
    Ok(())
}
