use anyhow::Result;
use std::sync::Arc;
use winit::{event::WindowEvent, window::Window};
use zenith::{
    App, Args, RenderContext, RenderableApp,
    renderer::NeuralMaterialRenderer,
    rendergraph::RenderGraphBuilder,
    rhi::{Descriptors, Gpu},
    ui::{Egui, egui},
};

#[path = "support/neural_material_validation.rs"]
mod validation;

struct NeuralMaterialApp {
    renderer: Option<NeuralMaterialRenderer>,
    ui: Option<Egui>,
    animate: bool,
    delta: f32,
}

impl App for NeuralMaterialApp {
    fn new(_args: &Args) -> Result<Self> {
        Ok(Self {
            renderer: None,
            ui: None,
            animate: false,
            delta: 0.0,
        })
    }

    fn on_window_event(&mut self, event: &WindowEvent, _window: &Window) {
        if let Some(ui) = &mut self.ui {
            ui.on_window_event(event);
        }
    }

    fn tick(&mut self, delta: f32) {
        self.delta = delta;
        if self.animate {
            if let Some(renderer) = &mut self.renderer {
                renderer.settings.rotation =
                    (renderer.settings.rotation + delta * 0.04).rem_euclid(1.0);
            }
        }
    }
}

impl RenderableApp for NeuralMaterialApp {
    fn prepare(
        &mut self,
        gpu: &Arc<Gpu>,
        _descriptors: &Arc<Descriptors>,
        window: Arc<Window>,
    ) -> Result<()> {
        window.set_title("Zenith | Neural material | Reference / Reconstruction");
        self.renderer = Some(NeuralMaterialRenderer::new(gpu)?);
        self.ui = Some(Egui::new(gpu, window)?);
        Ok(())
    }

    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> Result<()> {
        let renderer = self.renderer.as_mut().unwrap();
        let backends: Vec<_> = renderer.backends().collect();
        let mut settings = renderer.settings;
        let ui = self.ui.as_mut().unwrap();
        let frame = ui.run(|root| {
            egui::Window::new("Neural material")
                .default_pos([16.0, 16.0])
                .default_width(300.0)
                .resizable(true)
                .show(root, |ui| {
                    ui.label("Glazed stone · learned base color + roughness");
                    ui.label("Left: procedural reference   Right: neural reconstruction");
                    ui.separator();
                    egui::ComboBox::from_label("Inference")
                        .selected_text(settings.backend.label())
                        .show_ui(ui, |ui| {
                            for backend in &backends {
                                ui.selectable_value(
                                    &mut settings.backend,
                                    *backend,
                                    backend.label(),
                                );
                            }
                        });
                    ui.checkbox(&mut settings.flat_view, "UV material swatch");
                    ui.checkbox(&mut settings.error_view, "Reconstruction error ×12");
                    ui.checkbox(&mut self.animate, "Rotate material");
                    ui.add(egui::Slider::new(&mut settings.rotation, 0.0..=1.0).text("Rotation"));
                    ui.add(egui::Slider::new(&mut settings.exposure, 0.1..=3.0).text("Exposure"));
                    ui.small("16 Fourier features → 32 → 32 → RGB + roughness");
                    ui.small("ReLU MLP · pretrained weights · no external assets");
                    ui.small(format!("Frame {:.2} ms", self.delta * 1000.0));
                });
        });
        renderer.settings = settings;
        renderer.render(builder, context.output)?;
        ui.paint(builder, context.output, frame)
    }
}

fn main() -> Result<()> {
    if std::env::args().any(|arg| arg == "validate") {
        validation::run()
    } else {
        zenith::launch::<NeuralMaterialApp>()
    }
}
