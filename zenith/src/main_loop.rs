use crate::app::RenderableApp;
use crate::Engine;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};
use zenith_core::log::info;
use zenith_core::time::{Seconds, Timer};

pub struct EngineLoop<A> {
    app: A,
    engine: Option<Engine>,

    frame_count: u64,
    frame_timer: Timer,
    fps_timer: Timer,
    remaining_frames: Option<u64>,
    test_rendered: u64,
    test_resize: bool,
    restore_at: Option<std::time::Instant>,
}

impl<A: RenderableApp> ApplicationHandler for EngineLoop<A> {
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(engine) = &self.engine {
            if engine.should_exit() {
                event_loop.exit();
                engine.gpu.wait_idle().unwrap();
                return;
            }
            self.app
                .update_windows(event_loop)
                .expect("window update failed");
            let window = &self.engine.as_ref().unwrap().main_window;
            if window.is_minimized() != Some(true) {
                window.request_redraw();
            }
        }
        if self
            .restore_at
            .is_some_and(|time| std::time::Instant::now() >= time)
        {
            self.restore_at = None;
            if let Some(engine) = &self.engine {
                engine.main_window.set_minimized(false);
                engine.main_window.request_redraw();
            }
        }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window_size = LogicalSize::new(1920, 1080);

        // Calculate center position on primary monitor
        let position = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
            .map(|monitor| {
                let monitor_size = monitor.size();
                let monitor_pos = monitor.position();
                let scale_factor = monitor.scale_factor();

                // Convert logical window size to physical pixels
                let physical_window_width = (window_size.width as f64 * scale_factor) as i32;
                let physical_window_height = (window_size.height as f64 * scale_factor) as i32;

                let x = monitor_pos.x + (monitor_size.width as i32 - physical_window_width) / 2;
                let y = monitor_pos.y + (monitor_size.height as i32 - physical_window_height) / 2;
                PhysicalPosition::new(x, y)
            });

        let mut window_attributes = Window::default_attributes()
            .with_title("Zenith")
            .with_min_inner_size(LogicalSize::new(32, 32))
            .with_inner_size(window_size);

        if let Some(pos) = position {
            window_attributes = window_attributes.with_position(pos);
        }

        // TODO: only renderable app should create window
        let main_window = Arc::new(event_loop.create_window(window_attributes).unwrap());

        let engine = Engine::new(main_window.clone()).unwrap();

        self.app
            .prepare(&engine.gpu, &engine.descriptors, main_window.clone())
            .unwrap();
        self.engine = Some(engine);

        main_window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let engine = self.engine.as_mut().unwrap();
        if engine.should_exit() {
            event_loop.exit();
            engine.gpu.wait_idle().unwrap();
            return;
        }

        if window_id != engine.main_window.id() {
            self.app
                .on_auxiliary_window_event(window_id, &event)
                .expect("auxiliary window failed");
            return;
        }
        #[cfg(feature = "cpu-profiling")]
        let _capture = matches!(event, WindowEvent::RedrawRequested)
            .then(zenith_core::profile::cpu::begin_frame)
            .flatten();
        profiling::function_scope!();
        self.process_window_event(&event);
    }

    #[profiling::function]
    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        let engine = self.engine.as_mut().unwrap();
        if engine.should_exit() {
            event_loop.exit();
            engine.gpu.wait_idle().unwrap();
            return;
        }

        if engine.main_window.has_focus() {
            self.app.on_device_event(&event);
        }
    }
}

impl<A: RenderableApp> EngineLoop<A> {
    pub(super) fn new(app: A) -> Result<Self, anyhow::Error> {
        let mut frame_timer = Timer::new();
        frame_timer.start();
        let mut fps_timer = Timer::new();
        fps_timer.start();

        Ok(Self {
            engine: None,
            app,

            frame_count: 0u64,
            frame_timer,
            fps_timer,
            remaining_frames: std::env::var("ZENITH_TEST_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok()),
            test_rendered: 0,
            test_resize: std::env::var_os("ZENITH_TEST_RESIZE").is_some(),
            restore_at: None,
        })
    }

    pub fn run(mut self) -> Result<(), anyhow::Error> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        let instance = self
            .engine
            .as_ref()
            .map(|engine| engine.gpu.instance().clone());
        drop(self);
        if let Some(instance) = instance {
            anyhow::ensure!(
                instance.validation_errors().is_empty(),
                "Vulkan validation reported errors: {:?}",
                instance.validation_errors()
            );
        }
        Ok(())
    }

    #[profiling::function("main_loop")]
    fn process_window_event(&mut self, event: &WindowEvent) {
        self.app
            .on_window_event(event, self.engine.as_ref().unwrap().main_window.as_ref());

        match event {
            WindowEvent::Resized(_) => {
                let engine = self.engine.as_mut().unwrap();
                let app = &mut self.app;

                let inner_size = engine.main_window.inner_size();
                engine.resize(inner_size.width, inner_size.height).unwrap();
                app.resize(inner_size.width, inner_size.height);
            }
            WindowEvent::CloseRequested => {
                let engine = self.engine.as_mut().unwrap();

                engine.request_exit();
            }
            WindowEvent::RedrawRequested => {
                let update_start = std::time::Instant::now();
                self.tick();
                let update_time = update_start.elapsed();

                let engine = self.engine.as_mut().unwrap();
                let app = &mut self.app;

                let rendered = engine
                    .render_with_update_time(app, update_time)
                    .expect("rendering failed");
                if rendered {
                    self.test_rendered += 1;
                    if self.test_resize && self.test_rendered % 100 == 0 {
                        let size = if self.test_rendered % 200 == 0 {
                            (1280, 720)
                        } else {
                            (800, 450)
                        };
                        let _ = engine
                            .main_window
                            .request_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1));
                    }
                    if self.test_resize && self.test_rendered == 250 {
                        engine.main_window.set_minimized(true);
                        self.restore_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_millis(100));
                    }
                }
                if let Some(remaining) = &mut self.remaining_frames {
                    if rendered {
                        *remaining = remaining.saturating_sub(1);
                        if *remaining == 0 {
                            engine.request_exit();
                        }
                    }
                }
                engine.main_window.request_redraw();
            }
            _ => {}
        }
    }

    #[profiling::function]
    fn tick(&mut self) {
        self.frame_timer.tick();
        let delta_time = self.frame_timer.elapsed::<Seconds>().value() as f32;

        let last_time_print_elapsed = self.fps_timer.elapsed_total::<Seconds>().value() as f32;
        if last_time_print_elapsed > 1. {
            let fps: u32 = (self.frame_count as f32 / last_time_print_elapsed).ceil() as u32;
            let engine = self.engine.as_ref().unwrap();
            info!(
                "Frame rate: {} fps, pipelines: {}",
                fps,
                engine.pipeline_cache_size(),
            );
            self.fps_timer.reset();
            self.frame_count = 0;
        }

        let engine = self.engine.as_mut().unwrap();
        let app = &mut self.app;

        engine.tick(delta_time);
        {
            profiling::scope!("App update");
            app.tick(delta_time);
        }

        self.frame_count += 1;
    }
}
