pub use egui;

pub mod frame_timings;
pub mod gpu_timing;
mod renderer;

pub use frame_timings::FrameTimings;
pub use gpu_timing::GpuTimingGraph;
pub use renderer::UiRenderer;

use anyhow::Result;
use std::sync::Arc;
use winit::{event::WindowEvent, window::Window};
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::Gpu;

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
}

impl Egui {
    pub fn new(gpu: &Arc<Gpu>, window: Arc<Window>) -> Result<Self> {
        let context = egui::Context::default();
        context.set_embed_viewports(true);
        let mut platform = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        egui_winit::update_viewport_info(
            platform
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &context,
            &window,
            true,
        );
        Ok(Self {
            context,
            platform,
            renderer: UiRenderer::new(gpu)?,
            window,
        })
    }

    pub fn context(&self) -> &egui::Context {
        &self.context
    }

    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        let response = self.platform.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        response.consumed
    }

    pub fn run(&mut self, build: impl FnMut(&mut egui::Ui)) -> UiFrame {
        egui_winit::update_viewport_info(
            self.platform
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &self.context,
            &self.window,
            false,
        );
        let input = self.platform.take_egui_input(&self.window);
        let output = self.context.run_ui(input, build);
        self.platform
            .handle_platform_output(&self.window, output.platform_output);
        UiFrame {
            primitives: self
                .context
                .tessellate(output.shapes, output.pixels_per_point),
            textures: output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        }
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
