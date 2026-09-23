use super::*;
use crate::{DetachablePanel, Egui};
use winit::{
    application::ApplicationHandler, event_loop::EventLoop,
    platform::windows::EventLoopBuilderExtWindows,
};
use zenith_rhi::Instance;

#[derive(Default)]
struct Harness {
    result: Option<Result<()>>,
    instance: Option<Arc<Instance>>,
    ui: Option<Egui>,
    started: Option<Instant>,
    main_frames: u64,
    background_frames: u64,
}

impl Harness {
    fn exercise(&mut self, event_loop: &ActiveEventLoop) -> Result<Egui> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_inner_size(LogicalSize::new(640, 480)),
            )?,
        );
        let mut swapchain = Swapchain::new(window.clone(), true)?;
        let gpu = swapchain.gpu().clone();
        self.instance = Some(gpu.instance().clone());
        ensure!(
            gpu.instance().validation_enabled(),
            "validation is required"
        );
        let descriptors = Descriptors::new(&gpu, 2048, 32)?;
        let mut ui = Egui::new(&gpu, &descriptors, window)?;
        let mut panel = DetachablePanel::new("Rehosting test");
        let mut cache = ResourceCache::default();
        let mut pending = Vec::new();
        assert!(ui
            .detach(event_loop, "Invalid", egui::Rect::NOTHING)
            .is_err());
        assert!(!ui.is_detached());
        for cycle in 0..4 {
            let target = swapchain.acquire()?.expect("root frame");
            let frame = ui.run(|root| {
                panel.show(root, false, |ui| {
                    ui.label(format!("Root frame {cycle}"));
                })
            });
            let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
            let output = graph.import_frame(target.texture().clone())?;
            graph.pass(
                "background",
                vec![output.write(Access::COPY_WRITE)],
                move |ctx| {
                    ctx.commands
                        .clear_color(&ctx.image(output)?, [0.0, 0.0, 0.0, 1.0])
                },
            )?;
            ui.paint(&mut graph, output, frame)?;
            pending.push(target.present(graph.record()?)?);
            ui.detach(
                event_loop,
                "Rehosting test",
                egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(480.0, 360.0)),
            )?;
            assert!(ui.is_detached());
            assert!(Arc::ptr_eq(
                ui.detached.as_ref().unwrap().swapchain.gpu(),
                &gpu
            ));
            let child = ui.detached_window().unwrap().clone();
            assert!(!child.is_decorated());
            child.set_visible(false);
            ui.on_detached_window_event(&winit::event::WindowEvent::Focused(false));
            if cycle == 1 {
                ui.window.set_minimized(true);
                assert_eq!(ui.window.is_minimized(), Some(true));
            }
            let mut rendered = [0, 0];
            for index in 0..8 {
                std::thread::sleep(Duration::from_millis(20));
                assert!(ui.update_detached_window());
                if index == 4 {
                    let _ = child.request_inner_size(LogicalSize::new(520, 420));
                }
                if let Some(target) = ui.acquire_detached_frame()? {
                    let frame = ui.run(|root| {
                        panel.show(root, true, |ui| {
                            ui.label(format!("Detached frame {cycle} / {index}: αβγ"));
                            ui.add(egui::ProgressBar::new(0.5).text("Shared font atlas"));
                        })
                    });
                    ui.paint_detached(target, frame)?;
                    rendered[index / 4] += 1;
                }
            }
            assert!(rendered.iter().all(|count| *count > 0));
            child.set_minimized(true);
            assert!(!ui.update_detached_window());
            assert!(ui.acquire_detached_frame()?.is_none());
            child.set_minimized(false);
            ui.dock()?;
            assert!(!ui.is_detached());
            assert_ne!(ui.window.is_minimized(), Some(true));
            for submission in &mut pending {
                submission.wait(10_000_000_000)?;
            }
            pending.clear();
        }
        Ok(ui)
    }

    fn refresh_background_window(&mut self) -> Result<()> {
        let ui = self.ui.as_mut().unwrap();
        ui.window.request_redraw();
        if ui.update_detached_window() {
            if let Some(target) = ui.acquire_detached_frame()? {
                let frame = ui.run(|root| {
                    root.label(format!("Main frame {}", self.main_frames));
                });
                ui.paint_detached(target, frame)?;
                if ui.window.has_focus() && !ui.detached_window().unwrap().has_focus() {
                    assert!(ui.detached.as_ref().unwrap().floating);
                    self.background_frames += 1;
                }
            }
        }
        ensure!(
            self.started.unwrap().elapsed() < Duration::from_secs(5),
            "background redraw timed out: main {}, detached {}",
            self.main_frames,
            self.background_frames
        );
        Ok(())
    }
}

impl ApplicationHandler for Harness {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some(self.exercise(event_loop).and_then(|mut ui| {
            ui.window.set_visible(true);
            ui.detach(
                event_loop,
                "Background redraw test",
                egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(480.0, 360.0)),
            )?;
            ui.window.focus_window();
            self.ui = Some(ui);
            self.started = Some(Instant::now());
            Ok(())
        }));
        if self.result.as_ref().unwrap().is_err() {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.ui.is_none() {
            return;
        }
        self.result = Some(self.refresh_background_window());
        if self.result.as_ref().unwrap().is_err() || self.background_frames >= 12 {
            self.ui = None;
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        if let Some(ui) = &mut self.ui {
            if id == ui.window.id() {
                if matches!(event, winit::event::WindowEvent::RedrawRequested) {
                    self.main_frames += 1;
                    ui.window.request_redraw();
                }
            } else if ui.detached_window().is_some_and(|window| window.id() == id) {
                ui.on_detached_window_event(&event);
            }
        }
    }
}

#[test]
#[ignore = "requires Windows, Vulkan validation, and Slang"]
fn shared_gpu_windows_rehost_resize_and_retire_in_flight_frames() -> Result<()> {
    std::env::set_current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let mut harness = Harness::default();
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    event_loop.run_app(&mut harness)?;
    harness.result.take().expect("test ran")?;
    assert!(harness.background_frames >= 12);
    assert!(harness.main_frames > harness.background_frames);
    let instance = harness.instance.unwrap();
    ensure!(
        instance.validation_errors().is_empty(),
        "window validation: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
