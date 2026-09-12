use std::{collections::VecDeque, sync::Arc};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};
use zenith_core::log;
use zenith_rhi::*;

#[derive(Default)]
struct App {
    swapchain: Option<Swapchain>,
    window: Option<Arc<Window>>,
    submissions: VecDeque<Submission>,
    frames: u32,
    error: Option<anyhow::Error>,
    abandoned: bool,
    restore_at: Option<std::time::Instant>,
}

impl App {
    fn frame(&mut self) -> anyhow::Result<()> {
        let chain = self.swapchain.as_mut().unwrap();
        if self.submissions.len() >= 3 {
            self.submissions.pop_front().unwrap().wait(10_000_000_000)?;
        }
        let Some(frame) = chain.acquire()? else {
            return Ok(());
        };
        if self.frames == 333 && !self.abandoned {
            self.abandoned = true;
            drop(frame);
            return Ok(());
        }
        let mut commands = chain.gpu().commands()?;
        unsafe {
            commands.initialize(frame.texture())?;
        }
        if self.frames % 2 == 0 {
            commands.clear_color(frame.texture(), [0.2, 0.3, 0.8, 1.0])?;
        } else {
            let view = frame.texture().full_view()?;
            commands.begin_rendering(
                &[Attachment {
                    view: &view,
                    clear: Some([0.2, 0.3, 0.8, 1.0]),
                    store: true,
                    resolve: None,
                }],
                None,
                chain.extent(),
            )?;
            commands.end_rendering()?;
        }
        self.submissions.push_back(frame.present(commands)?);
        self.frames += 1;
        if self.frames == 250 {
            self.window.as_ref().unwrap().set_minimized(true);
            self.restore_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(100));
        }
        if self.frames % 100 == 0 {
            let size = if self.frames % 200 == 0 {
                (640, 360)
            } else {
                (800, 450)
            };
            let _ = self
                .window
                .as_ref()
                .unwrap()
                .request_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1));
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let result = || -> anyhow::Result<(Arc<Window>, Swapchain)> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("Zenith minimal Vulkan acceptance")
                        .with_inner_size(winit::dpi::PhysicalSize::new(640, 360)),
                )?,
            );
            let chain = Swapchain::new(window.clone(), true)?;
            anyhow::ensure!(
                chain.gpu().instance().validation_enabled(),
                "window acceptance requires validation layers"
            );
            Ok((window, chain))
        }();
        match result {
            Ok((window, chain)) => {
                window.request_redraw();
                self.window = Some(window);
                self.swapchain = Some(chain);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.frame() {
                    self.error = Some(error);
                    event_loop.exit();
                    return;
                }
                if self.frames
                    >= std::env::var("ZENITH_TEST_FRAMES")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1000)
                {
                    event_loop.exit();
                } else {
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(deadline) = self.restore_at {
            if std::time::Instant::now() >= deadline {
                self.restore_at = None;
                self.window.as_ref().unwrap().set_minimized(false);
                self.window.as_ref().unwrap().request_redraw();
            } else {
                event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(deadline));
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let mut app = App::default();
    EventLoop::new()?.run_app(&mut app)?;
    for mut submission in app.submissions.drain(..) {
        submission.wait(10_000_000_000)?;
    }
    if let Some(error) = app.error.take() {
        return Err(error);
    }
    let instance = app.swapchain.as_ref().unwrap().gpu().instance().clone();
    let frames = app.frames;
    drop(app);
    let instance_errors = instance.validation_errors();
    anyhow::ensure!(
        instance_errors.is_empty(),
        "validation errors: {}",
        instance_errors.join("\n")
    );
    log::info!(
        "PASS: {frames} frames, alternating transfer/rendering clear, resize, minimize/restore, abandoned acquire and shutdown"
    );
    Ok(())
}
