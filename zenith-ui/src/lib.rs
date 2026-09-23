pub use egui;

pub mod detachable_panel;
pub mod frame_timings;
pub mod gpu_timing;
mod native_window;
mod renderer;

pub use frame_timings::FrameTimings;
pub use gpu_timing::GpuTimingGraph;
#[cfg(feature = "cpu-profiling")]
pub mod cpu_profiler;
#[cfg(feature = "cpu-profiling")]
pub use cpu_profiler::CpuProfiler;
pub use detachable_panel::{DetachablePanel, PanelRequest};
pub use renderer::UiRenderer;

use anyhow::Result;
use native_window::{platform_state, NativeWindow};
use std::sync::Arc;
use winit::{event::WindowEvent, event_loop::ActiveEventLoop, window::Window};
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::{Descriptors, Frame, Gpu};

#[must_use = "pass the UI frame to Egui::paint"]
pub struct UiFrame {
    primitives: Vec<egui::ClippedPrimitive>,
    textures: egui::TexturesDelta,
    pixels_per_point: f32,
}

pub struct Egui {
    context: egui::Context,
    platform: egui_winit::State,
    renderer: UiRenderer,
    window: Arc<Window>,
    gpu: Arc<Gpu>,
    descriptors: Arc<Descriptors>,
    detached: Option<NativeWindow>,
}

impl Egui {
    pub fn new(
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        window: Arc<Window>,
    ) -> Result<Self> {
        let context = egui::Context::default();
        context.set_embed_viewports(true);
        let platform = platform_state(&context, &window);
        Ok(Self {
            context,
            platform,
            renderer: UiRenderer::new(gpu)?,
            window,
            gpu: gpu.clone(),
            descriptors: descriptors.clone(),
            detached: None,
        })
    }

    pub fn context(&self) -> &egui::Context {
        &self.context
    }

    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        if self.is_detached() {
            return false;
        }
        let response = self.platform.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        response.consumed
    }

    pub fn run(&mut self, build: impl FnMut(&mut egui::Ui)) -> UiFrame {
        let (platform, window) = match &mut self.detached {
            Some(detached) => (&mut detached.platform, &detached.window),
            None => (&mut self.platform, &self.window),
        };
        egui_winit::update_viewport_info(
            platform
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &self.context,
            window,
            false,
        );
        let input = platform.take_egui_input(window);
        let output = self.context.run_ui(input, build);
        platform.handle_platform_output(window, output.platform_output);
        UiFrame {
            primitives: self
                .context
                .tessellate(output.shapes, output.pixels_per_point),
            textures: output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        }
    }

    pub fn is_detached(&self) -> bool {
        self.detached.is_some()
    }

    pub fn detached_window(&self) -> Option<&Arc<Window>> {
        self.detached.as_ref().map(|window| &window.window)
    }

    pub fn detach(
        &mut self,
        event_loop: &ActiveEventLoop,
        title: &str,
        rect: egui::Rect,
    ) -> Result<()> {
        if self.is_detached() {
            return Ok(());
        }
        self.detached = Some(NativeWindow::new(
            event_loop,
            &self.gpu,
            &self.context,
            &self.window,
            title,
            rect,
        )?);
        Ok(())
    }

    pub fn dock(&mut self) -> Result<()> {
        if let Some(detached) = &mut self.detached {
            detached.finish()?;
            detached.window.set_visible(false);
        }
        self.detached = None;
        self.platform = platform_state(&self.context, &self.window);
        self.window.set_minimized(false);
        self.window.focus_window();
        self.window.request_redraw();
        Ok(())
    }

    pub fn update_detached_window(&mut self) -> bool {
        self.detached
            .as_mut()
            .is_some_and(|detached| detached.update(&self.window))
    }

    pub fn on_detached_window_event(&mut self, event: &WindowEvent) {
        if let Some(detached) = &mut self.detached {
            let response = detached.platform.on_window_event(&detached.window, event);
            if response.repaint {
                detached.request_repaint();
            }
        }
    }

    pub fn acquire_detached_frame(&mut self) -> Result<Option<Frame>> {
        match &mut self.detached {
            Some(detached) => detached.acquire(),
            None => Ok(None),
        }
    }

    pub fn paint_detached(&mut self, target: Frame, frame: UiFrame) -> Result<()> {
        let detached = self.detached.as_mut().expect("detached frame has a window");
        detached.paint(
            &self.gpu,
            &self.descriptors,
            &mut self.renderer,
            target,
            frame,
        )
    }

    pub fn paint(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        output: ImageId,
        frame: UiFrame,
    ) -> Result<()> {
        self.renderer.paint(
            builder,
            output,
            frame.pixels_per_point,
            frame.primitives,
            frame.textures,
        )
    }
}
