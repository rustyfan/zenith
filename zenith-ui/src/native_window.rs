use crate::{UiFrame, UiRenderer};
use anyhow::{ensure, Result};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
    event_loop::ActiveEventLoop,
    window::{Window, WindowLevel},
};
use zenith_rendergraph::{RenderGraphBuilder, ResourceCache};
use zenith_rhi::{Access, Descriptors, Frame, Gpu, Submission, Swapchain};

pub(crate) fn platform_state(context: &egui::Context, window: &Arc<Window>) -> egui_winit::State {
    let mut platform = egui_winit::State::new(
        context.clone(),
        egui::ViewportId::ROOT,
        window.as_ref(),
        Some(window.scale_factor() as f32),
        window.theme(),
        None,
    );
    platform
        .egui_input_mut()
        .events
        .push(egui::Event::WindowFocused(false));
    platform.egui_input_mut().focused = window.has_focus();
    egui_winit::update_viewport_info(
        platform
            .egui_input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default(),
        context,
        window,
        true,
    );
    platform
}

#[derive(Default)]
struct FrameSlot {
    submission: Option<Submission>,
    cache: ResourceCache,
}

pub(crate) struct NativeWindow {
    pub window: Arc<Window>,
    pub platform: egui_winit::State,
    swapchain: Swapchain,
    frames: [FrameSlot; 3],
    index: usize,
    next_redraw: Instant,
    repaint_requested: bool,
    floating: bool,
}

impl NativeWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        gpu: &Arc<Gpu>,
        context: &egui::Context,
        parent: &Arc<Window>,
        title: &str,
        rect: egui::Rect,
    ) -> Result<Self> {
        ensure!(
            rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0,
            "invalid UI window bounds"
        );
        let origin = parent.inner_position()?;
        let pixels_per_point = context.pixels_per_point() as f64;
        let mut position = PhysicalPosition::new(
            origin.x + (rect.min.x as f64 * pixels_per_point).round() as i32,
            origin.y + (rect.min.y as f64 * pixels_per_point).round() as i32,
        );
        let monitors: Vec<_> = event_loop.available_monitors().collect();
        if let Some(monitor) = monitors.iter().min_by_key(|monitor| {
            let min = monitor.position();
            let size = monitor.size();
            let dx = (position.x as i64
                - (position.x as i64).clamp(min.x as i64, min.x as i64 + size.width as i64))
            .abs();
            let dy = (position.y as i64
                - (position.y as i64).clamp(min.y as i64, min.y as i64 + size.height as i64))
            .abs();
            dx + dy
        }) {
            let min = monitor.position();
            let size = monitor.size();
            position.x = position
                .x
                .clamp(min.x, min.x + size.width.saturating_sub(100) as i32);
            position.y = position
                .y
                .clamp(min.y, min.y + size.height.saturating_sub(100) as i32);
        }
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(format!("{title} — Tools"))
                    .with_decorations(false)
                    .with_visible(false)
                    .with_position(position)
                    .with_inner_size(LogicalSize::new(
                        rect.width().max(280.0) * context.zoom_factor(),
                        rect.height().max(200.0) * context.zoom_factor(),
                    ))
                    .with_min_inner_size(LogicalSize::new(280.0, 200.0)),
            )?,
        );
        let swapchain = Swapchain::with_gpu(window.clone(), gpu)?;
        let platform = platform_state(context, &window);
        window.set_visible(true);
        window.focus_window();
        window.request_redraw();
        Ok(Self {
            window,
            platform,
            swapchain,
            frames: std::array::from_fn(|_| FrameSlot::default()),
            index: 0,
            next_redraw: Instant::now(),
            repaint_requested: true,
            floating: false,
        })
    }

    pub fn update(&mut self, parent: &Window) -> bool {
        let floating = parent.has_focus() || self.window.has_focus();
        if self.floating != floating {
            self.window.set_window_level(if floating {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });
            self.floating = floating;
        }
        self.window.is_minimized() != Some(true)
            && (self.repaint_requested || Instant::now() >= self.next_redraw)
    }

    pub fn request_repaint(&mut self) {
        self.repaint_requested = true;
    }

    pub fn acquire(&mut self) -> Result<Option<Frame>> {
        self.repaint_requested = false;
        self.next_redraw = Instant::now() + Duration::from_millis(16);
        if let Some(mut submission) = self.frames[self.index].submission.take() {
            submission.wait(10_000_000_000)?;
        }
        self.swapchain.acquire()
    }

    pub fn paint(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        renderer: &mut UiRenderer,
        target: Frame,
        frame: UiFrame,
    ) -> Result<()> {
        let slot = &mut self.frames[self.index];
        let mut graph = RenderGraphBuilder::new(gpu, descriptors, &mut slot.cache)?;
        let output = graph.import_frame(target.texture().clone())?;
        graph.pass(
            "ui_window_clear",
            vec![output.write(Access::COPY_WRITE)],
            move |ctx| {
                ctx.commands
                    .clear_color(&ctx.image(output)?, [0.02, 0.02, 0.02, 1.0])
            },
        )?;
        renderer.paint(
            &mut graph,
            output,
            frame.pixels_per_point,
            frame.primitives,
            frame.textures,
        )?;
        slot.submission = Some(target.present(graph.record()?)?);
        self.index = (self.index + 1) % self.frames.len();
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        for slot in &mut self.frames {
            if let Some(mut submission) = slot.submission.take() {
                submission.wait(10_000_000_000)?;
            }
            slot.cache.clear();
        }
        Ok(())
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("UI window cleanup failed: {error:#}");
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests;
