use zenith::rendergraph::{Access, RenderGraphBuilder};
use zenith::{launch, App, Args, RenderContext, RenderableApp};

pub struct SimpleApp;
impl App for SimpleApp {
    fn new(_args: &Args) -> anyhow::Result<Self> {
        Ok(Self)
    }
}
impl RenderableApp for SimpleApp {
    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> anyhow::Result<()> {
        let output = context.output;
        builder.pass(
            "clear",
            vec![output.write(Access::COPY_WRITE)],
            move |ctx| {
                ctx.commands
                    .clear_color(&ctx.image(output)?, [0.2, 0.3, 0.8, 1.0])
            },
        )
    }
}
fn main() {
    launch::<SimpleApp>().expect("Failed to launch zenith engine loop!");
}
