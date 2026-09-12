//! Zenith world space coordinate system (right-hand side, z up)
//!
//!                z
//!                ^    y
//!                |   /
//!                |  /
//!                | /
//!                ----------> x
//!

use glam::{EulerRot, Mat4, Quat, Vec3, Vec4};
use crate::log::warn;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::window::{CursorGrabMode, Window};
use crate::math::{Degree, Radians};

pub const NEAR_PLANE: f32 = 0.1;
pub const WORLD_SPACE_UP: Vec3 = Vec3::new(0., 0., 1.);
pub const WORLD_SPACE_FORWARD: Vec3 = Vec3::new(0., 1., 0.);
pub const WORLD_SPACE_RIGHT: Vec3 = Vec3::new(1., 0., 0.);

#[derive(Debug, Clone)]
pub struct ViewData {
    pub view_proj: Mat4,
    pub inv_view_proj: Mat4,
    pub view: Mat4,
    pub inv_view: Mat4,
    pub proj: Mat4,
    pub inv_proj: Mat4,
    pub position: Vec4,
}

/// Common camera data.
#[derive(Debug)]
pub struct Camera {
    position: Vec3,
    rotation: Quat,
    pitch: Radians,
    yaw: Radians,

    forward: Vec3,
    right: Vec3,
    up: Vec3,
    /// horizontal FOV
    fov: Degree,
    aspect_ratio: f32,
    z_near: f32,
    view: Mat4,
    proj: Mat4,
}

impl Default for Camera {
    fn default() -> Self {
        let aspect_ratio = 16.0 / 9.0;
        let fov = Degree::new(90.0);
        let mut cam = Self {
            position: Default::default(),
            rotation: Quat::IDENTITY,
            pitch: Default::default(),
            yaw: Default::default(),

            forward: WORLD_SPACE_FORWARD,
            right: WORLD_SPACE_RIGHT,
            up: WORLD_SPACE_UP,

            fov,
            aspect_ratio,
            z_near: NEAR_PLANE,

            view: Default::default(),
            proj: Default::default(),
        };
        cam.update_view();
        cam.update_proj();
        cam
    }
}

impl Camera {
    pub fn new(fov: Degree, aspect_ratio: f32, z_near: f32) -> Self {
        let mut cam = Self {
            fov,
            aspect_ratio,
            z_near,
            ..Default::default()
        };
        cam.update_view();
        cam.update_proj();
        cam
    }

    #[inline]
    pub fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    #[inline]
    pub fn set_aspect_ratio(&mut self, aspect_ratio: f32) {
        if self.aspect_ratio != aspect_ratio {
            self.aspect_ratio = aspect_ratio;
            self.update_proj();
        }
    }

    #[inline]
    pub fn set_fov(&mut self, fov: Degree) {
        if self.fov != fov {
            self.fov = fov;
            self.update_proj();
        }
    }

    /// Return the location of camera.
    #[inline]
    pub fn position(&self) -> Vec3 {
        self.position
    }

    /// Return the view matrix of this camera.
    #[inline]
    pub fn view(&self) -> Mat4 { self.view }

    /// Return the projection matrix of this camera.
    #[inline]
    pub fn projection(&self) -> Mat4 {
        self.proj
    }

    /// Return the view-projection matrix of this camera.
    #[inline]
    pub fn view_projection(&self) -> Mat4 {
        self.proj * self.view
    }

    /// Return the forward vector of this camera.
    #[inline]
    pub fn forward(&self) -> Vec3 {
        self.forward
    }

    /// Return the right vector of this camera.
    #[inline]
    pub fn right(&self) -> Vec3 {
        self.right
    }

    /// Return the up vector of this camera.
    #[inline]
    pub fn up(&self) -> Vec3 {
        self.up
    }

    pub fn view_data(&self) -> ViewData {
        let vp = self.view_projection();
        let v = self.view();
        let p = self.projection();
        ViewData {
            view_proj: vp,
            inv_view_proj: vp.inverse(),
            view: v,
            inv_view: v.inverse(),
            proj: p,
            inv_proj: p.inverse(),
            position: Vec4::new(self.position().x, self.position().y, self.position().z, 1.0),
        }
    }

    fn translate(&mut self, delta_position: Vec3) {
        let r = self.right();
        let f = self.forward();
        let u = self.up();

        self.position += r * delta_position.x + f * delta_position.y + u * delta_position.z;
    }

    fn rotate(&mut self, delta_yaw: Radians, delta_pitch: Radians, max_pitch: Radians) {
        self.yaw += delta_yaw;
        self.pitch += delta_pitch;
        self.pitch = self.pitch.clamp(-max_pitch, max_pitch);
        // eliminate roll and avoid gimbal lock
        self.rotation = Quat::from_euler(EulerRot::ZXY, self.yaw.into(), self.pitch.into(), 0.);
    }

    fn update_view(&mut self) {
        let forward = self.forward();
        self.view = Mat4::look_to_rh(self.position, forward, WORLD_SPACE_UP);
    }

    fn update_proj(&mut self) {
        let half_fov: Radians = (self.fov / 2.0).into();
        let fov_y = Radians::new((half_fov.tan() / self.aspect_ratio).atan() * 2.0);
        self.proj = Mat4::perspective_infinite_reverse_rh(*fov_y, self.aspect_ratio, self.z_near);
    }

    fn update_local_basis(&mut self) {
        self.forward = self.rotation * WORLD_SPACE_FORWARD;
        self.right = self.rotation * WORLD_SPACE_RIGHT;
        self.up = self.rotation * WORLD_SPACE_UP;
    }
}

/// Controller to modify specific camera data.
pub struct CameraController {
    accum_local_pitch: Radians,
    max_pitch_angle: Radians,
    accum_local_yaw: Radians,

    move_speed: f32,
    mouse_sensitivity: f32,
    /// The higher the value, the higher the lagging. Zero results in abrupt changes.
    rotation_smoothing_factor: f32,

    accum_dx: f32,
    accum_dy: f32,
    is_grabbed: bool,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            accum_local_pitch: Default::default(),
            max_pitch_angle: Degree::from(89.99).into(),
            accum_local_yaw: Default::default(),

            move_speed: 70.,
            mouse_sensitivity: 30.0,
            rotation_smoothing_factor: 0.5,

            accum_dx: 0.0,
            accum_dy: 0.0,
            is_grabbed: false,
        }
    }
}

impl CameraController {
    pub fn new(mouse_sensitivity: f32) -> Self {
        Self {
            mouse_sensitivity,
            ..Default::default()
        }
    }

    /// The higher the value, the smoother the rotation.
    pub fn set_rotation_smoothing_factor(&mut self, rotation_smoothing_factor: f32) {
        self.rotation_smoothing_factor = rotation_smoothing_factor;
    }

    /// Determine how fast camera location changes.
    pub fn set_move_speed(&mut self, move_speed: f32) {
        self.move_speed = move_speed;
    }

    /// Determine how fast camera rotation changes.
    pub fn set_mouse_sensitivity(&mut self, mouse_sensitivity: f32) {
        self.mouse_sensitivity = mouse_sensitivity;
    }

    /// Receive and process window events.
    #[profiling::function]
    pub fn on_window_event(&mut self, event: &WindowEvent, window: &Window) {
        match event {
            WindowEvent::MouseInput { button, state, .. } => {
                if *button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            self.grab_cursor(window);
                        }
                        ElementState::Released => {
                            self.release_cursor(window);
                        }
                    }
                }
            }
            WindowEvent::Focused(false) => {
                // release cursor when window loses focus
                self.release_cursor(window);
            }
            _ => {}
        }
    }

    /// Receive and process device events.
    #[profiling::function]
    pub fn on_device_event(&mut self, event: &DeviceEvent) {
        match event {
            DeviceEvent::MouseMotion { delta } => {
                if self.is_grabbed {
                    self.accum_dx += delta.0 as f32;
                    self.accum_dy += delta.1 as f32;
                }
            }
            _ => {}
        }
    }

    /// Update camera with axis speed.
    #[profiling::function]
    pub fn update_cameras<'a>(
        &mut self,
        delta_time: f32,
        forward_axis_speed: f32,
        right_axis_speed: f32,
        up_axis_speed: f32,
        to_update_cameras: impl IntoIterator<Item = &'a mut Camera>
    ) {
        let d_local_yaw = Radians::from(-self.accum_dx * self.mouse_sensitivity * delta_time);
        let d_local_pitch = Radians::from(-self.accum_dy * self.mouse_sensitivity * delta_time);

        let blend_factor = 1.0 - self.rotation_smoothing_factor.powf(delta_time * 60.0);

        self.accum_local_yaw += d_local_yaw;
        self.accum_local_pitch += d_local_pitch;

        let delta_yaw = self.accum_local_yaw * blend_factor;
        let delta_pitch = self.accum_local_pitch * blend_factor;

        self.accum_local_yaw -= delta_yaw;
        self.accum_local_pitch -= delta_pitch;

        let axis_dir = Vec3::new(
            right_axis_speed,
            forward_axis_speed,
            up_axis_speed,
        );
        let delta_pos = axis_dir * self.move_speed * delta_time;

        for camera in to_update_cameras {
            camera.rotate(delta_yaw, delta_pitch, self.max_pitch_angle);
            camera.translate(delta_pos);
            camera.update_local_basis();
            camera.update_view();
        }

        self.accum_dx = 0.0;
        self.accum_dy = 0.0;
    }

    fn grab_cursor(&mut self, window: &Window) {
        self.is_grabbed = true;

        window.set_cursor_visible(false);

        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            if window.set_cursor_grab(CursorGrabMode::Confined).is_err() {
                warn!("Failed to grab cursor.")
            }
        }
    }

    fn release_cursor(&mut self, window: &Window) {
        self.is_grabbed = false;
        window.set_cursor_visible(true);

        if window.set_cursor_grab(CursorGrabMode::None).is_err() {
            warn!("Failed to release cursor.")
        }
    }
}
