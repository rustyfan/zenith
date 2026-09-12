use glam::Vec3;
use std::path::PathBuf;
use std::sync::Arc;
use winit::event::{DeviceEvent, WindowEvent};
use winit::keyboard::KeyCode;
use winit::window::Window;

use zenith::asset::manager::AssetRequestor;
use zenith::asset::mesh::Scene;
use zenith::asset::{AssetHandle, AssetLoadRequest};
use zenith::core::camera::{Camera, CameraController, NEAR_PLANE};
use zenith::core::input::InputActionMapper;
use zenith::core::log;
use zenith::core::math::Degree;
use zenith::core::time::{Milliseconds, Timer};
use zenith::renderer::{DebugMode, WorldRenderer};
use zenith::rendergraph::RenderGraphBuilder;
use zenith::rhi::{Descriptors, Gpu};
use zenith::{launch, App, Args, RenderContext, RenderableApp};

pub struct WorldApp {
    world_renderer: Option<WorldRenderer>,
    input: InputActionMapper,
    camera: Camera,
    controller: CameraController,
    asset_requestor: AssetRequestor,
}

impl App for WorldApp {
    fn new(_args: &Args) -> Result<Self, anyhow::Error> {
        Ok(Self {
            world_renderer: None,
            input: InputActionMapper::new(),
            camera: Camera::default(),
            controller: CameraController::new(10.0),
            asset_requestor: AssetRequestor::new(),
        })
    }

    fn on_window_event(&mut self, event: &WindowEvent, window: &Window) {
        self.input.on_window_event(event);
        self.controller.on_window_event(event, window);
    }

    fn on_device_event(&mut self, event: &DeviceEvent) {
        self.controller.on_device_event(event);
    }

    fn tick(&mut self, delta_time: f32) {
        self.input.tick(delta_time);

        let forward = self.input.get_axis("walk");
        let right = self.input.get_axis("strafe");
        let up = self.input.get_axis("lift");

        self.controller.update_cameras(
            delta_time,
            forward,
            right,
            up,
            std::iter::once(&mut self.camera),
        );

        if self.input.is_action_just_pressed("toggle_diffuse_sh") {
            if let Some(ref mut renderer) = self.world_renderer {
                static mut TOGGLE: bool = false;

                let toggle = unsafe {
                    TOGGLE = !TOGGLE;
                    TOGGLE
                };
                if toggle {
                    renderer.set_debug_mode(DebugMode::DIFFUSE_SH);
                } else {
                    renderer.set_debug_mode(DebugMode::empty());
                }
            }
        }
    }
}

impl RenderableApp for WorldApp {
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

        let size = window.inner_size();
        let aspect = if size.height == 0 {
            1.0
        } else {
            (size.width as f32) / (size.height as f32)
        };
        let mut camera = Camera::new(Degree::from(90.0), aspect, NEAR_PLANE);
        camera.set_position(Vec3::new(0.0, -90.0, 0.0));
        self.camera = camera;

        let mut load_timer = Timer::new();
        load_timer.start();
        self.asset_requestor.request_load(
            AssetLoadRequest::new("mesh/cerberus/scene.scene")
                .with_source("mesh/cerberus/scene.gltf"),
        )?;
        self.asset_requestor.request_load(
            AssetLoadRequest::new("texture/minedump_flats_4k.tex")
                .with_source("texture/minedump_flats_4k.hdr"),
        )?;
        load_timer.stop();
        let load_ms = load_timer.elapsed_total::<Milliseconds>().value();

        let mut renderer_new_timer = Timer::new();
        renderer_new_timer.start();
        let mut renderer = WorldRenderer::new(render_device, descriptors, size.width, size.height)?;
        renderer_new_timer.stop();
        let renderer_new_ms = renderer_new_timer.elapsed_total::<Milliseconds>().value();

        let scene = AssetHandle::<Scene>::new(PathBuf::from("mesh/cerberus/scene.scene").into());
        let mut upload_timer = Timer::new();
        upload_timer.start();
        renderer.add_scene(render_device, descriptors, scene)?;

        let skybox_handle = AssetHandle::<zenith::asset::texture::Texture>::new(
            PathBuf::from("texture/minedump_flats_4k.tex").into(),
        );
        if let Some(skybox_tex) = skybox_handle.get() {
            renderer.set_skybox(render_device, descriptors, &skybox_tex)?;
            log::info!("Skybox loaded and set successfully");
        } else {
            log::warn!("Skybox texture not found after loading HDR");
        }

        upload_timer.stop();
        let upload_ms = upload_timer.elapsed_total::<Milliseconds>().value();

        self.world_renderer = Some(renderer);

        prepare_timer.stop();
        let prepare_ms = prepare_timer.elapsed_total::<Milliseconds>().value();
        log::info!(
            "WorldApp prepare timings: prepare={:.3}ms, request_load={:.3}ms, world_renderer_new={:.3}ms, gpu_upload={:.3}ms",
            prepare_ms,
            load_ms,
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
        self.world_renderer.as_mut().unwrap().resize(width, height);
    }

    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder,
        context: RenderContext,
    ) -> anyhow::Result<()> {
        self.world_renderer
            .as_mut()
            .unwrap()
            .render(builder, &self.camera, context.output)
    }
}

fn main() {
    launch::<WorldApp>().expect("Failed to launch zenith engine loop!");
}
