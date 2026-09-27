use anyhow::Result;
use glam::Vec3;
use std::sync::Arc;
use winit::{
    event::{DeviceEvent, WindowEvent},
    keyboard::KeyCode,
    window::Window,
};
use zenith::{
    asset::{AssetServer, MemorySource},
    core::{
        camera::{Camera, CameraController, NEAR_PLANE},
        input::InputActionMapper,
        math::Degree,
    },
    renderer::{DebugMode, WorldRenderer},
    rendergraph::RenderGraphBuilder,
    rhi::{Descriptors, Gpu},
    ui::{egui, Egui},
    App, Args, RenderContext, RenderableApp,
};

#[path = "support/einar_scene.rs"]
mod einar;
#[path = "support/bxdf_scene.rs"]
mod scene;
#[path = "support/bxdf_validation.rs"]
mod validation;

struct BxdfLab {
    renderer: Option<WorldRenderer>,
    ui: Option<Egui>,
    assets: AssetServer,
    camera: Camera,
    controller: CameraController,
    input: InputActionMapper,
    angle: f32,
    rotate: bool,
    debug: DebugMode,
    timings: Vec<(String, f64)>,
    delta: f32,
    einar: bool,
}
impl App for BxdfLab {
    fn new(args: &Args) -> Result<Self> {
        Ok(Self {
            renderer: None,
            ui: None,
            assets: AssetServer::builder()
                .source(MemorySource::default())
                .with_builtin_assets()
                .build()?,
            camera: Camera::default(),
            controller: CameraController::default(),
            input: InputActionMapper::new(),
            angle: -0.8,
            rotate: false,
            debug: DebugMode::empty(),
            timings: Vec::new(),
            delta: 0.0,
            einar: args.args.iter().any(|arg| arg == "einar"),
        })
    }
    fn on_window_event(&mut self, event: &WindowEvent, window: &Window) {
        let consumed = self.ui.as_mut().is_some_and(|ui| ui.on_window_event(event));
        let release = matches!(
            event,
            WindowEvent::KeyboardInput {
                event: winit::event::KeyEvent {
                    state: winit::event::ElementState::Released,
                    ..
                },
                ..
            } | WindowEvent::MouseInput {
                state: winit::event::ElementState::Released,
                ..
            } | WindowEvent::Focused(false)
        );
        if !consumed || release {
            self.input.on_window_event(event);
            self.controller.on_window_event(event, window);
        }
    }
    fn on_device_event(&mut self, event: &DeviceEvent) {
        self.controller.on_device_event(event);
    }
    fn tick(&mut self, delta: f32) {
        self.delta = delta;
        if self.rotate {
            self.angle += delta * 0.35;
        }
        self.input.tick(delta);
        self.controller.update_cameras(
            delta,
            self.input.get_axis("walk"),
            self.input.get_axis("strafe"),
            self.input.get_axis("lift"),
            std::iter::once(&mut self.camera),
        );
    }
}
impl RenderableApp for BxdfLab {
    fn prepare(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        window: Arc<Window>,
    ) -> Result<()> {
        window.set_title("Zenith | BxDF lab | Clear coat and Marschner hair");
        let size = window.inner_size();
        self.camera = Camera::new(
            Degree::new(60.0),
            size.width as f32 / size.height.max(1) as f32,
            NEAR_PLANE,
        );
        self.camera.set_position(if self.einar {
            Vec3::new(0.0, -5.5, 0.0)
        } else {
            Vec3::new(-0.3, -17.5, -0.5)
        });
        self.controller
            .update_cameras(0.0, 0.0, 0.0, 0.0, std::iter::once(&mut self.camera));
        self.input
            .register_axis("walk", [KeyCode::KeyW], [KeyCode::KeyS], 0.2);
        self.input
            .register_axis("strafe", [KeyCode::KeyD], [KeyCode::KeyA], 0.2);
        self.input
            .register_axis("lift", [KeyCode::KeyE], [KeyCode::KeyQ], 0.2);
        let mut renderer = WorldRenderer::new(gpu, descriptors, size.width, size.height)?;
        let model = if self.einar {
            einar::scene(
                &self.assets,
                std::path::Path::new("content/mesh/einar/einar.bin"),
            )?
        } else {
            scene::scene(&self.assets)
        };
        renderer.add_scene(gpu, descriptors, &model)?;
        renderer.set_skybox(gpu, descriptors, &scene::environment(&self.assets, false))?;
        let mut light = renderer.lighting_settings();
        light.ambient_occlusion.enabled = false;
        light.directional.intensity = 3.0;
        light.sky_intensity = 0.4;
        renderer.set_lighting(light)?;
        self.renderer = Some(renderer);
        self.ui = Some(Egui::new(gpu, descriptors, window)?);
        Ok(())
    }
    fn resize(&mut self, width: u32, height: u32) {
        if height > 0 {
            self.camera.set_aspect_ratio(width as f32 / height as f32);
        }
    }
    fn gpu_timing_enabled(&self) -> bool {
        true
    }
    fn on_gpu_timings(&mut self, _: u64, _: std::time::Instant, passes: &[(String, f64)]) {
        self.timings = passes.to_vec();
    }
    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> Result<()> {
        let renderer = self.renderer.as_mut().unwrap();
        let mut lighting = renderer.lighting_settings();
        let mut hair = renderer.hair_settings();
        let mut post = renderer.post_processing_settings();
        let frame = self.ui.as_mut().unwrap().run(|root| {
            egui::Window::new("BxDF laboratory")
                .default_pos([12.0, 12.0])
                .default_width(260.0)
                .vscroll(true)
                .show(root, |ui| {
                    if self.einar {
                        ui.label("Einar - hair shading study");
                        ui.small("Einar Rig (CC-BY) Blender Foundation");
                        ui.hyperlink_to(
                            "studio.blender.org",
                            "https://studio.blender.org/characters/einar/v1/",
                        );
                        ui.small("Neutral skin - original strand groom");
                    } else {
                        ui.label("Coat: weight → / roughness ↓");
                        ui.label("Bottom: independent base / coat normals");
                        ui.label("Hair: blond → brown → dark");
                        ui.label("Top: straight - Bottom: curved / overlapping");
                    }
                    ui.separator();
                    ui.checkbox(&mut self.rotate, "Rotate light");
                    ui.add(egui::Slider::new(&mut self.angle, -3.14..=3.14).text("Light angle"));
                    ui.horizontal(|ui| {
                        if ui.button("Front light").clicked() {
                            self.angle = -1.57;
                        }
                        if ui.button("Back light").clicked() {
                            self.angle = 1.57;
                        }
                    });
                    ui.add(
                        egui::Slider::new(&mut lighting.directional.intensity, 0.0..=8.0)
                            .text("Direct"),
                    );
                    ui.add(
                        egui::Slider::new(&mut lighting.sky_intensity, 0.0..=2.0)
                            .text("Environment"),
                    );
                    ui.add(egui::Slider::new(&mut post.exposure, 0.1..=4.0).text("Exposure"));
                    ui.checkbox(&mut lighting.shadows.enabled, "Shadows");
                    ui.checkbox(&mut hair.self_shadows, "Hair transmission shadows");
                    for (flag, label) in [
                        (DebugMode::WHITE_FURNACE, "White environment"),
                        (DebugMode::COAT_ONLY, "Coat only"),
                        (DebugMode::BASE_ONLY, "Base only"),
                        (DebugMode::HAIR_TANGENT, "Strand tangents"),
                    ] {
                        let mut enabled = self.debug.contains(flag);
                        if ui.checkbox(&mut enabled, label).changed() {
                            self.debug.set(flag, enabled);
                        }
                    }
                    ui.separator();
                    ui.label("Hair scattering");
                    ui.checkbox(&mut hair.gaussian_reference, "Marschner Gaussian reference");
                    ui.small("Regularized azimuth; corrected longitudinal default");
                    ui.horizontal(|ui| {
                        for (bit, label) in [(1, "R"), (2, "TT"), (4, "TRT"), (8, "Higher orders")]
                        {
                            let mut enabled = hair.lobes & bit != 0;
                            if ui.checkbox(&mut enabled, label).changed() {
                                if enabled {
                                    hair.lobes |= bit;
                                } else {
                                    hair.lobes &= !bit;
                                }
                            }
                        }
                    });
                    ui.checkbox(&mut hair.reference_quadrature, "64-point azimuth reference");
                    ui.add(
                        egui::Slider::new(&mut hair.environment_samples, 8..=256)
                            .text("Environment samples"),
                    );
                    ui.separator();
                    if ui.button("Reset camera").clicked() {
                        self.camera.set_position(if self.einar {
                            Vec3::new(0.0, -5.5, 0.0)
                        } else {
                            Vec3::new(-0.3, -17.5, -0.5)
                        });
                        self.controller = CameraController::default();
                    }
                    ui.small("Left mouse on scene + WASD / QE: camera");
                    ui.label(format!("Frame {:.2} ms", self.delta * 1000.0));
                    for (name, ms) in &self.timings {
                        if name.contains("hair") || name == "lighting" {
                            ui.label(format!("{name}: {ms:.2} ms"));
                        }
                    }
                });
        });
        lighting.directional.direction_to_light =
            Vec3::new(self.angle.cos(), self.angle.sin(), 0.35).normalize();
        renderer.set_lighting(lighting)?;
        renderer.set_hair_settings(hair)?;
        renderer.set_post_processing(post)?;
        renderer.set_debug_mode(self.debug);
        renderer.render(builder, &self.camera, context.output)?;
        self.ui
            .as_mut()
            .unwrap()
            .paint(builder, context.output, frame)
    }
}
fn main() -> Result<()> {
    if std::env::args().any(|arg| arg == "validate") {
        validation::run()
    } else {
        zenith::launch::<BxdfLab>()
    }
}
