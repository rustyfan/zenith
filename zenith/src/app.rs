use std::sync::Arc;
use winit::{
    event::{DeviceEvent, WindowEvent},
    window::Window,
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
    fn prepare(
        &mut self,
        _gpu: &Arc<Gpu>,
        _descriptors: &Arc<Descriptors>,
        _window: Arc<Window>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn resize(&mut self, _width: u32, _height: u32) {}
    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> anyhow::Result<()>;
}
