use std::sync::Arc;
use winit::window::Window;
use zenith::renderer::TriangleRenderer;
use zenith::rendergraph::RenderGraphBuilder;
use zenith::rhi::{Descriptors, Gpu};
use zenith::{launch, App, Args, RenderContext, RenderableApp};

pub struct TriangleApp {
    renderer: Option<TriangleRenderer>,
}
impl App for TriangleApp {
    fn new(_args: &Args) -> anyhow::Result<Self> {
        Ok(Self { renderer: None })
    }
}
impl RenderableApp for TriangleApp {
    fn prepare(
        &mut self,
        gpu: &Arc<Gpu>,
        _descriptors: &Arc<Descriptors>,
        _window: Arc<Window>,
    ) -> anyhow::Result<()> {
        self.renderer = Some(TriangleRenderer::new(gpu)?);
        Ok(())
    }
    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> anyhow::Result<()> {
        self.renderer
            .as_mut()
            .unwrap()
            .render(builder, context.output)
    }
}
fn main() {
    launch::<TriangleApp>().expect("Failed to launch zenith engine loop!");
}
