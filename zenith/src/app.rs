use std::{sync::Arc, time::Instant};
use winit::{
    event::{DeviceEvent, WindowEvent},
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};
use zenith_core::cli::EngineArgs;
use zenith_rendergraph::{ImageId, RenderGraphBuilder};
use zenith_rhi::{vk, Descriptors, Gpu};

pub trait App: Sized + 'static {
    fn new(args: &EngineArgs) -> anyhow::Result<Self>;
    fn on_window_event(&mut self, _event: &WindowEvent, _window: &Window) {}
    fn on_device_event(&mut self, _event: &DeviceEvent) {}
    fn tick(&mut self, _delta_time: f32) {}
}

pub struct RenderContext {
    pub output: ImageId,
    pub extent: vk::Extent2D,
    pub frame_index: usize,
}

pub trait RenderableApp: App {
    fn update_windows(&mut self, _event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        Ok(())
    }
    fn on_auxiliary_window_event(
        &mut self,
        _window_id: WindowId,
        _event: &WindowEvent,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn prepare(
        &mut self,
        _gpu: &Arc<Gpu>,
        _descriptors: &Arc<Descriptors>,
        _window: Arc<Window>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn resize(&mut self, _width: u32, _height: u32) {}
    fn gpu_timing_enabled(&self) -> bool {
        false
    }
    fn on_gpu_timings(
        &mut self,
        _frame_number: u64,
        _captured_at: Instant,
        _passes: &[(String, f64)],
    ) {
    }
    fn on_frame_timings(&mut self, _frame_number: u64, _cpu_ms: f64, _gpu_ms: Option<f64>) {}
    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> anyhow::Result<()>;
}
