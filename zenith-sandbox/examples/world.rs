#[path = "support/pbr_scene.rs"]
mod pbr_scene;

#[cfg(test)]
#[path = "support/world_tests.rs"]
mod tests;

use glam::Vec3;
use std::sync::Arc;
use winit::event::{DeviceEvent, ElementState, WindowEvent};
use winit::keyboard::KeyCode;
use winit::window::Window;

use zenith::asset::mesh::{Mesh, Scene};
use zenith::asset::{texture::Texture, AssetServer, CpuRetention, FileSource, Handle};
use zenith::core::camera::{Camera, CameraController, NEAR_PLANE};
use zenith::core::input::InputActionMapper;
use zenith::core::log;
use zenith::core::math::Degree;
use zenith::core::time::{Milliseconds, Timer};
use zenith::renderer::{DebugMode, SceneStatus, WorldRenderer};
use zenith::rendergraph::RenderGraphBuilder;
use zenith::rhi::{Descriptors, Gpu};
#[cfg(feature = "cpu-profiling")]
use zenith::ui::CpuProfiler;
use zenith::ui::{egui, Egui, FrameTimings, GpuTimingGraph, UiFrame};
use zenith::{launch, App, Args, RenderContext, RenderableApp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum UiTab {
    Controls,
    GpuTimings,
    FrameTimings,
    #[cfg(feature = "cpu-profiling")]
    CpuProfiler,
}

pub struct WorldApp {
    world_renderer: Option<WorldRenderer>,
    input: InputActionMapper,
    camera: Camera,
    alternate_camera: Camera,
    controller: CameraController,
    assets: AssetServer,
    skybox: Handle<Texture>,
    first_frame_rendered: bool,
    model_requested: bool,
    model_to_frame: Option<Handle<Scene>>,
    model_index: Option<usize>,
    cerberus_index: Option<usize>,
    sphere_index: Option<usize>,
    showing_spheres: bool,
    debug_mode: DebugMode,
    window: Option<Arc<Window>>,
    ui: Option<Egui>,
    ui_tab: UiTab,
    gpu_timing: Option<GpuTimingGraph>,
    frame_timings: FrameTimings,
    #[cfg(feature = "cpu-profiling")]
    cpu_profiler: CpuProfiler,
    frame_time: f32,
}

impl WorldApp {
    fn ui_frame(&mut self) -> anyhow::Result<Option<UiFrame>> {
        zenith::core::profile::scope!("UI build");
        let (Some(ui), Some(renderer)) = (&mut self.ui, &mut self.world_renderer) else {
            return Ok(None);
        };
        let mut lighting = renderer.lighting_settings();
        let mut post_processing = renderer.post_processing_settings();
        let mut diffuse_sh = self.debug_mode.contains(DebugMode::DIFFUSE_SH);
        let mut ambient_occlusion = self.debug_mode.contains(DebugMode::AMBIENT_OCCLUSION);
        let mut white_furnace = self.debug_mode.contains(DebugMode::WHITE_FURNACE);
        let mut switch_scene = false;
        let frame = ui.run(|root| {
            egui::Window::new("Zenith")
                .default_pos([16.0, 16.0])
                .default_size([600.0, 460.0])
                .min_width(280.0)
                .show(root, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.ui_tab, UiTab::Controls, "Controls");
                        ui.selectable_value(&mut self.ui_tab, UiTab::GpuTimings, "GPU timings");
                        ui.selectable_value(&mut self.ui_tab, UiTab::FrameTimings, "Frame timings");
                        #[cfg(feature = "cpu-profiling")]
                        ui.selectable_value(&mut self.ui_tab, UiTab::CpuProfiler, "CPU profiler");
                    });
                    ui.separator();
                    ui.label(format!(
                        "{:.0} FPS  |  {:.2} ms",
                        1.0 / self.frame_time.max(0.000001),
                        self.frame_time * 1000.0
                    ));
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(self.ui_tab)
                        .auto_shrink([false, false])
                        .show(ui, |ui| match self.ui_tab {
                            UiTab::Controls => {
                                ui.add(
                                    egui::Slider::new(&mut post_processing.exposure, 0.0..=8.0)
                                        .text("Exposure"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut lighting.directional.intensity,
                                        0.0..=8.0,
                                    )
                                    .text("Directional"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut lighting.sky_intensity, 0.0..=8.0)
                                        .text("Skylight"),
                                );
                                ui.checkbox(&mut lighting.shadows.enabled, "Directional shadows");
                                ui.checkbox(
                                    &mut lighting.ambient_occlusion.enabled,
                                    "Ambient occlusion",
                                );
                                ui.checkbox(
                                    &mut lighting.multiple_scattering,
                                    "Multiple scattering",
                                );
                                ui.separator();
                                ui.checkbox(&mut diffuse_sh, "Diffuse SH view");
                                ui.checkbox(&mut ambient_occlusion, "AO view");
                                ui.checkbox(&mut white_furnace, "White furnace");
                                ui.separator();
                                switch_scene |= ui
                                    .button(if self.showing_spheres {
                                        "Show Cerberus"
                                    } else {
                                        "Show spheres"
                                    })
                                    .clicked();
                                ui.small("Drag outside this panel to look around.");
                                ui.small("WASD / Q / E to move.");
                            }
                            UiTab::GpuTimings => {
                                let mut enabled = self.gpu_timing.is_some();
                                if ui.checkbox(&mut enabled, "Enable pass graph").changed() {
                                    self.gpu_timing = enabled.then(GpuTimingGraph::default);
                                }
                                if let Some(graph) = &mut self.gpu_timing {
                                    graph.show(ui);
                                }
                            }
                            UiTab::FrameTimings => {
                                self.frame_timings.show(ui);
                            }
                            #[cfg(feature = "cpu-profiling")]
                            UiTab::CpuProfiler => {
                                self.cpu_profiler.show(ui);
                            }
                        });
                });
        });
        renderer.set_lighting(lighting)?;
        renderer.set_post_processing(post_processing)?;
        self.debug_mode.set(DebugMode::DIFFUSE_SH, diffuse_sh);
        self.debug_mode
            .set(DebugMode::AMBIENT_OCCLUSION, ambient_occlusion);
        self.debug_mode.set(DebugMode::WHITE_FURNACE, white_furnace);
        renderer.set_debug_mode(self.debug_mode);
        if switch_scene {
            self.toggle_scene()?;
        }
        if self.controller.is_cursor_grabbed() {
            if let Some(window) = &self.window {
                window.set_cursor_visible(false);
            }
        }
        Ok(Some(frame))
    }

    fn toggle_scene(&mut self) -> anyhow::Result<()> {
        let Some(renderer) = &mut self.world_renderer else {
            return Ok(());
        };
        let sphere_index = *self
            .sphere_index
            .get_or_insert_with(|| renderer.queue_scene(&pbr_scene::spheres(&self.assets)));
        let showing_spheres = !self.showing_spheres;
        renderer.set_scene_visible(sphere_index, showing_spheres)?;
        if let Some(index) = self.cerberus_index {
            renderer.set_scene_visible(index, !showing_spheres)?;
        }
        self.showing_spheres = showing_spheres;
        std::mem::swap(&mut self.camera, &mut self.alternate_camera);
        self.controller
            .set_move_speed(if showing_spheres { 5.0 } else { 70.0 });
        log::info!(
            "{}",
            if showing_spheres {
                "Sphere comparison: roughness 0 to 1 left to right; metallic 0, 0.5, 1 top to bottom (T: Cerberus)"
            } else {
                "Cerberus (T: sphere comparison)"
            }
        );
        Ok(())
    }
}

impl App for WorldApp {
    fn new(_args: &Args) -> Result<Self, anyhow::Error> {
        let assets = AssetServer::builder()
            .source(FileSource::new(
                std::env::var_os("ZENITH_CONTENT").unwrap_or_else(|| "content".into()),
            ))
            .cache_dir(std::env::var_os("ZENITH_ASSET_CACHE").unwrap_or_else(|| "asset".into()))
            .with_builtin_assets()
            .cpu_retention::<Mesh>(CpuRetention::ReleaseAfterUpload)
            .cpu_retention::<Texture>(CpuRetention::ReleaseAfterUpload)
            .build()?;
        let skybox = assets.load::<Texture>("texture/minedump_flats_4k.hdr")?;
        Ok(Self {
            world_renderer: None,
            input: InputActionMapper::new(),
            camera: Camera::default(),
            alternate_camera: Camera::default(),
            controller: CameraController::new(10.0),
            first_frame_rendered: false,
            model_requested: false,
            model_to_frame: None,
            model_index: None,
            cerberus_index: None,
            sphere_index: None,
            showing_spheres: false,
            debug_mode: DebugMode::empty(),
            window: None,
            ui: None,
            ui_tab: UiTab::Controls,
            gpu_timing: Some(GpuTimingGraph::default()),
            frame_timings: FrameTimings::default(),
            #[cfg(feature = "cpu-profiling")]
            cpu_profiler: CpuProfiler::default(),
            frame_time: 1.0 / 60.0,
            assets,
            skybox,
        })
    }

    fn on_window_event(&mut self, event: &WindowEvent, window: &Window) {
        let consumed = self.ui.as_mut().is_some_and(|ui| ui.on_window_event(event));
        let release = matches!(
            event,
            WindowEvent::KeyboardInput {
                event: winit::event::KeyEvent {
                    state: ElementState::Released,
                    ..
                },
                ..
            } | WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } | WindowEvent::Focused(false)
        );
        if consumed && !release {
            return;
        }
        self.input.on_window_event(event);
        self.controller.on_window_event(event, window);
    }

    fn on_device_event(&mut self, event: &DeviceEvent) {
        if self
            .ui
            .as_ref()
            .is_some_and(|ui| ui.context().egui_wants_pointer_input())
        {
            return;
        }
        self.controller.on_device_event(event);
    }

    fn tick(&mut self, delta_time: f32) {
        self.frame_time += (delta_time - self.frame_time) * 0.1;
        if self.first_frame_rendered && !self.model_requested {
            self.model_requested = true;
            match self.assets.load::<Scene>("mesh/cerberus/scene.gltf") {
                Ok(scene) => {
                    let renderer = self.world_renderer.as_mut().unwrap();
                    let index = renderer.queue_scene(&scene);
                    if let Err(error) = renderer.set_scene_visible(index, !self.showing_spheres) {
                        log::error!("Scene visibility: {error}");
                    }
                    self.model_index = Some(index);
                    self.cerberus_index = Some(index);
                    self.model_to_frame = Some(scene);
                    log::info!("Model requested after skybox-only first frame");
                }
                Err(error) => log::error!("Model request failed: {error}"),
            }
        }
        self.input.tick(delta_time);

        let keyboard_captured = self
            .ui
            .as_ref()
            .is_some_and(|ui| ui.context().egui_wants_keyboard_input());
        let forward = if keyboard_captured {
            0.0
        } else {
            self.input.get_axis("walk")
        };
        let right = if keyboard_captured {
            0.0
        } else {
            self.input.get_axis("strafe")
        };
        let up = if keyboard_captured {
            0.0
        } else {
            self.input.get_axis("lift")
        };

        self.controller.update_cameras(
            delta_time,
            forward,
            right,
            up,
            std::iter::once(&mut self.camera),
        );
        if keyboard_captured {
            return;
        }
        if self.input.is_action_just_pressed("toggle_scene") {
            if let Err(error) = self.toggle_scene() {
                log::error!("Scene switch: {error}");
            }
        }

        if self.input.is_action_just_pressed("toggle_diffuse_sh") {
            self.debug_mode.toggle(DebugMode::DIFFUSE_SH);
        }
        if self.input.is_action_just_pressed("debug_ao") {
            self.debug_mode.toggle(DebugMode::AMBIENT_OCCLUSION);
        }
        let toggle_furnace = self.input.is_action_just_pressed("toggle_furnace");
        let toggle_multiple_scattering = self
            .input
            .is_action_just_pressed("toggle_multiple_scattering");
        if toggle_furnace {
            self.debug_mode.toggle(DebugMode::WHITE_FURNACE);
            log::info!(
                "White furnace: {}",
                self.debug_mode.contains(DebugMode::WHITE_FURNACE)
            );
        }
        if let Some(renderer) = &mut self.world_renderer {
            renderer.set_debug_mode(self.debug_mode);
            let mut lighting = renderer.lighting_settings();
            if toggle_multiple_scattering {
                lighting.multiple_scattering = !lighting.multiple_scattering;
                log::info!("Multiple scattering: {}", lighting.multiple_scattering);
            }
            if self.input.is_action_just_pressed("toggle_ao") {
                lighting.ambient_occlusion.enabled = !lighting.ambient_occlusion.enabled;
                log::info!("Ambient occlusion: {}", lighting.ambient_occlusion.enabled);
            }
            if self.input.is_action_just_pressed("toggle_shadows") {
                lighting.shadows.enabled = !lighting.shadows.enabled;
                log::info!("Directional shadows: {}", lighting.shadows.enabled);
            }
            if self.input.is_action_just_pressed("toggle_directional") {
                lighting.directional.intensity = if lighting.directional.intensity > 0.0 {
                    0.0
                } else {
                    1.0
                };
            }
            if self.input.is_action_just_pressed("toggle_skylight") {
                lighting.sky_intensity = if lighting.sky_intensity > 0.0 {
                    0.0
                } else {
                    1.0
                };
            }
            if let Err(error) = renderer.set_lighting(lighting) {
                log::error!("Lighting settings: {error}");
            }
        }
    }
}

impl RenderableApp for WorldApp {
    fn gpu_timing_enabled(&self) -> bool {
        self.ui.is_some()
            && (self.ui_tab == UiTab::FrameTimings
                || self
                    .gpu_timing
                    .as_ref()
                    .is_some_and(|graph| !graph.is_paused()))
    }

    fn on_frame_timings(&mut self, frame_number: u64, cpu_ms: f64, gpu_ms: Option<f64>) {
        self.frame_timings.set_frame(frame_number, cpu_ms, gpu_ms);
    }

    fn on_gpu_timings(
        &mut self,
        frame_number: u64,
        captured_at: std::time::Instant,
        passes: &[(String, f64)],
    ) {
        if let Some(graph) = &mut self.gpu_timing {
            graph.push_frame(frame_number, captured_at, passes);
        }
    }

    fn prepare(
        &mut self,
        render_device: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        window: Arc<Window>,
    ) -> anyhow::Result<()> {
        let mut prepare_timer = Timer::new();
        prepare_timer.start();

        self.input
            .register_axis("walk", [KeyCode::KeyW], [KeyCode::KeyS], 0.2);
        self.input
            .register_axis("strafe", [KeyCode::KeyD], [KeyCode::KeyA], 0.2);
        self.input
            .register_axis("lift", [KeyCode::KeyE], [KeyCode::KeyQ], 0.2);
        self.input
            .register_action("toggle_diffuse_sh", [KeyCode::KeyM]);
        self.input
            .register_action("toggle_directional", [KeyCode::KeyL]);
        self.input
            .register_action("toggle_skylight", [KeyCode::KeyI]);
        self.input.register_action("toggle_scene", [KeyCode::KeyT]);
        self.input.register_action("toggle_ao", [KeyCode::KeyO]);
        self.input.register_action("debug_ao", [KeyCode::KeyN]);
        self.input
            .register_action("toggle_shadows", [KeyCode::KeyH]);
        self.input
            .register_action("toggle_furnace", [KeyCode::KeyF]);
        self.input
            .register_action("toggle_multiple_scattering", [KeyCode::KeyC]);
        log::info!("O: ambient occlusion; N: AO debug view; H: directional shadows");
        log::info!(
            "F: white furnace debug view; C: multiple-scattering compensation; T: switch scene"
        );

        let size = window.inner_size();
        let aspect = if size.height == 0 {
            1.0
        } else {
            (size.width as f32) / (size.height as f32)
        };
        let mut camera = Camera::new(Degree::from(90.0), aspect, NEAR_PLANE);
        camera.set_position(Vec3::new(0.0, -90.0, 0.0));
        self.camera = camera;
        let mut comparison_camera = Camera::new(Degree::from(60.0), aspect, NEAR_PLANE);
        let distance = 6.1f32.max(2.95 * aspect) / 30.0f32.to_radians().tan() * 1.15;
        comparison_camera.set_position(Vec3::new(0.0, -distance, 0.0));
        self.controller
            .update_cameras(0.0, 0.0, 0.0, 0.0, std::iter::once(&mut comparison_camera));
        self.alternate_camera = comparison_camera;

        let mut load_timer = Timer::new();
        load_timer.start();
        let skybox = &self.skybox;

        let mut renderer_new_timer = Timer::new();
        renderer_new_timer.start();
        let mut renderer = WorldRenderer::new(render_device, descriptors, size.width, size.height)?
            .with_asset_server(&self.assets);
        renderer.set_staging_cache_budget(0);
        renderer_new_timer.stop();
        let renderer_new_ms = renderer_new_timer.elapsed_total::<Milliseconds>().value();

        let mut wait_timer = Timer::new();
        wait_timer.start();
        skybox.wait()?;
        wait_timer.stop();
        let wait_ms = wait_timer.elapsed_total::<Milliseconds>().value();
        load_timer.stop();
        let ready_ms = load_timer.elapsed_total::<Milliseconds>().value();
        let mut upload_timer = Timer::new();
        upload_timer.start();
        renderer.set_skybox(render_device, descriptors, skybox)?;
        log::info!("Asset CPU counters: {:?}", self.assets.stats());
        log::info!("Asset GPU counters: {:?}", renderer.upload_stats());
        self.assets.watch(Some(std::time::Duration::from_secs(1)));

        upload_timer.stop();
        let upload_ms = upload_timer.elapsed_total::<Milliseconds>().value();

        self.world_renderer = Some(renderer);
        self.ui = Some(Egui::new(render_device, window.clone())?);
        self.window = Some(window);

        prepare_timer.stop();
        let prepare_ms = prepare_timer.elapsed_total::<Milliseconds>().value();
        log::info!(
            "WorldApp prepare timings: prepare={:.3}ms, renderer_and_asset_wait={:.3}ms, asset_wait_after_renderer={:.3}ms, world_renderer_new={:.3}ms, gpu_upload={:.3}ms",
            prepare_ms,
            ready_ms,
            wait_ms,
            renderer_new_ms,
            upload_ms
        );
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.camera.set_aspect_ratio(width as f32 / height as f32);
        self.alternate_camera
            .set_aspect_ratio(width as f32 / height as f32);
        self.world_renderer.as_mut().unwrap().resize(width, height);
    }

    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder,
        context: RenderContext,
    ) -> anyhow::Result<()> {
        if let Some(handle) = &self.model_to_frame {
            let camera = if self.showing_spheres {
                &mut self.alternate_camera
            } else {
                &mut self.camera
            };
            let framed = handle.with_snapshot(|scene| {
                let Some(scene) = scene else {
                    return false;
                };
                let mut minimum = Vec3::splat(f32::INFINITY);
                let mut maximum = Vec3::splat(f32::NEG_INFINITY);
                for instance in &scene.instances {
                    let Some(mesh) = instance.mesh.get() else {
                        return false;
                    };
                    let transform = glam::Mat4::from_cols_array(&instance.transform);
                    for vertex in &mesh.vertices {
                        let position =
                            transform.transform_point3(Vec3::from_array(vertex.position));
                        minimum = minimum.min(position);
                        maximum = maximum.max(position);
                    }
                }
                if minimum.is_finite() && maximum.is_finite() {
                    let center = (minimum + maximum) * 0.5;
                    let distance = (maximum - minimum).length().max(1.0) * 1.25;
                    camera.set_position(center - Vec3::Y * distance);
                    self.controller.update_cameras(
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        std::iter::once(&mut *camera),
                    );
                }
                true
            });
            if framed || handle.last_error().is_some() {
                self.model_to_frame = None;
            }
        }
        let ui_frame = self.ui_frame()?;
        let renderer = self.world_renderer.as_mut().unwrap();
        renderer.render(builder, &self.camera, context.output)?;
        if let (Some(ui), Some(frame)) = (&mut self.ui, ui_frame) {
            ui.paint(builder, context.output, frame)?;
        }
        self.first_frame_rendered = true;
        if let Some(index) = self.model_index {
            match renderer.scene_status(index) {
                Some(SceneStatus::Ready { .. }) => {
                    log::info!(
                        "Streamed model ready; CPU: {:?}; GPU: {:?}",
                        self.assets.stats(),
                        renderer.upload_stats()
                    );
                    self.model_index = None;
                }
                Some(SceneStatus::Failed { .. }) => self.model_index = None,
                _ => {}
            }
        }
        Ok(())
    }
}

fn main() {
    launch::<WorldApp>().expect("Failed to launch zenith engine loop!");
}
