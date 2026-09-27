use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use crate::collections::hashmap::HashMap;
use crate::collections::hashset::HashSet;
use crate::collections::SmallVec;

/// Key state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// KeyCode was just pressed this frame
    JustPressed,
    /// KeyCode is being held down
    Held,
    /// KeyCode was just released this frame
    JustReleased,
    /// KeyCode is not pressed
    Released,
}

/// In duty of collecting input events from OS.
/// User can query key input state for current frame.
pub struct InputManager {
    keys_pressed: HashSet<KeyCode>,
    keys_just_pressed: HashSet<KeyCode>,
    keys_just_released: HashSet<KeyCode>,
    keys_with_repeat: HashSet<KeyCode>,
    prev_keys_pressed: HashSet<KeyCode>,

    mouse_pressed: HashSet<MouseButton>,
    mouse_just_pressed: HashSet<MouseButton>,
    mouse_just_released: HashSet<MouseButton>,
    prev_mouse_pressed: HashSet<MouseButton>,

    modifiers: ModifiersState,
}

/// Modifier of this frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct ModifiersState {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            keys_pressed: HashSet::new(),
            keys_just_pressed: HashSet::new(),
            keys_just_released: HashSet::new(),
            keys_with_repeat: HashSet::new(),
            prev_keys_pressed: HashSet::new(),

            mouse_pressed: HashSet::new(),
            mouse_just_pressed: HashSet::new(),
            mouse_just_released: HashSet::new(),
            prev_mouse_pressed: HashSet::new(),

            modifiers: ModifiersState::default(),
        }
    }

    /// Receive and process window events.
    pub fn on_window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(keycode) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if !event.repeat {
                                // only register as pressed if it's not a repeat event
                                self.keys_pressed.insert(keycode);
                                self.keys_with_repeat.remove(&keycode);
                            } else {
                                // mark this key as having repeat events
                                self.keys_with_repeat.insert(keycode);
                            }
                        }
                        ElementState::Released => {
                            self.keys_pressed.remove(&keycode);
                            self.keys_with_repeat.remove(&keycode);
                        }
                    }
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                match state {
                    ElementState::Pressed => {
                        self.mouse_pressed.insert(*button);
                    }
                    ElementState::Released => {
                        self.mouse_pressed.remove(button);
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = ModifiersState {
                    shift: modifiers.state().shift_key(),
                    ctrl: modifiers.state().control_key(),
                    alt: modifiers.state().alt_key(),
                    super_key: modifiers.state().super_key(),
                };
            }
            WindowEvent::Focused(false) => {
                // clear all input when window loses focus
                self.clear();
            }
            _ => {}
        }
    }

    /// Update input states.
    pub fn tick(&mut self) {
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_just_pressed.clear();
        self.mouse_just_released.clear();

        for key in &self.keys_pressed {
            if !self.prev_keys_pressed.contains(key) {
                self.keys_just_pressed.insert(*key);
            }
        }

        for key in &self.prev_keys_pressed {
            if !self.keys_pressed.contains(key) {
                self.keys_just_released.insert(*key);
            }
        }

        for button in &self.mouse_pressed {
            if !self.prev_mouse_pressed.contains(button) {
                self.mouse_just_pressed.insert(*button);
            }
        }

        for button in &self.prev_mouse_pressed {
            if !self.mouse_pressed.contains(button) {
                self.mouse_just_released.insert(*button);
            }
        }

        self.prev_keys_pressed = self.keys_pressed.clone();
        self.prev_mouse_pressed = self.mouse_pressed.clone();
    }

    /// Query the state of a key.
    pub fn key_state(&self, key: KeyCode) -> KeyState {
        if self.keys_just_pressed.contains(&key) {
            KeyState::JustPressed
        } else if self.keys_pressed.contains(&key) {
            KeyState::Held
        } else if self.keys_just_released.contains(&key) {
            KeyState::JustReleased
        } else {
            KeyState::Released
        }
    }

    /// Return true if a key is pressed.
    pub fn is_key_pressed(&self, key: KeyCode) -> bool {
        self.keys_pressed.contains(&key)
    }

    /// Return true if a key is just pressed (i.e. key turns from unpress to press in this frame)
    pub fn is_key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys_just_pressed.contains(&key)
    }

    /// Return true if a key is just released (i.e. key turns from press to unpress in this frame)
    pub fn is_key_just_released(&self, key: KeyCode) -> bool {
        self.keys_just_released.contains(&key)
    }

    /// Return true if a key is held (i.e. key is pressed but NOT just pressed)
    pub fn is_key_held(&self, key: KeyCode) -> bool {
        self.keys_pressed.contains(&key) && !self.keys_just_pressed.contains(&key)
    }

    /// Return all pressed keys.
    pub fn pressed_keys(&self) -> &HashSet<KeyCode> {
        &self.keys_pressed
    }

    /// Return true if a mouse button is pressed.
    pub fn is_mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse_pressed.contains(&button)
    }

    /// Return true if a mouse button is just pressed. (i.e. mouse button turns from unpress to press in this frame)
    pub fn is_mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_just_pressed.contains(&button)
    }

    /// Return true if a mouse button is just pressed. (i.e. mouse button turns from press to unpress in this frame)
    pub fn is_mouse_just_released(&self, button: MouseButton) -> bool {
        self.mouse_just_released.contains(&button)
    }

    /// Return the state of modifier key in this frame.
    pub fn modifiers(&self) -> &ModifiersState {
        &self.modifiers
    }

    /// Clear all inner states.
    /// Useful to reset all input events.
    pub fn clear(&mut self) {
        self.keys_pressed.clear();
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.keys_with_repeat.clear();
        self.mouse_pressed.clear();
        self.mouse_just_pressed.clear();
        self.mouse_just_released.clear();
    }
}

/// Map input events into meaningful input action.
/// Current support:
///     Input action (single key pressed, 0 or 1)
///     Axis action  (1D direction vector represents by float between [-1, 1])
pub struct InputActionMapper {
    input: InputManager,
    action_mappings: HashMap<String, SmallVec<[KeyCode; 1]>>,
    axis_mappings: HashMap<String, AxisMapping>,
}

/// Directional, non-abrupt changes mapping useful to do movement mapping.
#[derive(Debug, Clone)]
pub struct AxisMapping {
    positive: SmallVec<[KeyCode; 1]>,
    negative: SmallVec<[KeyCode; 1]>,
    axis: f32,
    frame_axis: f32,
    /// The higher the value, the higher the lagging. Zero fallbacks to abrupt change.
    smoothing_factor: f32,
}

impl InputActionMapper {
    pub fn new() -> Self {
        Self {
            input: InputManager::new(),
            action_mappings: HashMap::new(),
            axis_mappings: HashMap::new(),
        }
    }

    /// Register an action mapping.
    pub fn register_action(&mut self, action: &str, keys: impl IntoIterator<Item = KeyCode>) {
        self.action_mappings.insert(action.to_string(), keys.into_iter().collect::<SmallVec<_>>());
    }

    /// Register an axis mapping.
    pub fn register_axis(&mut self, axis: &str, positive: impl IntoIterator<Item = KeyCode>, negative: impl IntoIterator<Item = KeyCode>, smoothing_factor: f32) {
        self.axis_mappings.insert(
            axis.to_string(),
            AxisMapping {
                positive: positive.into_iter().collect::<SmallVec<_>>(),
                negative: negative.into_iter().collect::<SmallVec<_>>(),
                axis: 0.0,
                frame_axis: 0.0,
                smoothing_factor,
            }
        );
    }

    /// Receive and process window events.
    #[profiling::function]
    pub fn on_window_event(&mut self, event: &WindowEvent) {
        self.input.on_window_event(event);
    }

    /// Update input mapping states.
    #[profiling::function]
    pub fn tick(&mut self, delta_time: f32) {
        self.input.tick();

        for mapping in self.axis_mappings.values_mut() {
            let positive = mapping.positive.iter().any(|key| self.input.is_key_pressed(*key));
            let negative = mapping.negative.iter().any(|key| self.input.is_key_pressed(*key));
            let target = (positive as i32 - negative as i32) as f32;
            let smoothing = mapping.smoothing_factor.clamp(0.0, 1.0);
            if delta_time <= 0.0 || smoothing == 1.0 {
                mapping.frame_axis = mapping.axis;
            } else if smoothing == 0.0 {
                mapping.axis = target;
                mapping.frame_axis = target;
            } else {
                let decay = -20.0 * smoothing.ln() * delta_time;
                let blend = -(-decay).exp_m1();
                // Integrate the exponential response over the frame to obtain average velocity.
                mapping.frame_axis = target + (mapping.axis - target) * (blend / decay);
                mapping.axis += (target - mapping.axis) * blend;
            }
        }
    }

    /// Return true if a specific action is pressed.
    pub fn is_action_pressed(&self, action: &str) -> bool {
        if let Some(keys) = self.action_mappings.get(action) {
            keys.iter().any(|key| self.input.is_key_pressed(*key))
        } else {
            false
        }
    }

    /// Return true if a specific action is just pressed. (i.e. action turns from unpress to press in this frame)
    pub fn is_action_just_pressed(&self, action: &str) -> bool {
        if let Some(keys) = self.action_mappings.get(action) {
            keys.iter().any(|key| self.input.is_key_just_pressed(*key))
        } else {
            false
        }
    }

    /// Return a float in [-1, 1] represents the direction and strength for a specific axis mapping.
    pub fn get_axis(&self, axis: &str) -> f32 {
        if let Some(mapping) = self.axis_mappings.get(axis) {
            mapping.frame_axis
        } else {
            0.0
        }
    }

    /// Get the inner input manager in order to query raw input events.
    pub fn raw_input(&self) -> &InputManager {
        &self.input
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, CameraController};

    fn move_camera(fps: usize, smoothing: f32) -> f32 {
        let mut input = InputActionMapper::new();
        input.register_axis("walk", [KeyCode::KeyW], [KeyCode::KeyS], smoothing);
        let mut camera = Camera::default();
        let mut controller = CameraController::default();
        controller.set_move_speed(1.0);
        for key in [Some(KeyCode::KeyW), None, Some(KeyCode::KeyS)] {
            input.input.keys_pressed.clear();
            if let Some(key) = key {
                input.input.keys_pressed.insert(key);
            }
            for _ in 0..fps {
                let dt = 1.0 / fps as f32;
                input.tick(dt);
                controller.update_cameras(dt, input.get_axis("walk"), 0.0, 0.0, [&mut camera]);
            }
        }
        camera.position().y
    }

    #[test]
    fn smoothed_translation_is_independent_of_frame_rate() {
        for smoothing in [0.0, 0.5, 0.99, 1.0] {
            let reference = move_camera(60, smoothing);
            for fps in [20, 30, 144, 240] {
                assert!((move_camera(fps, smoothing) - reference).abs() < 1e-5, "{fps} FPS");
            }
        }
    }

    #[test]
    fn opposite_keys_cancel_without_resetting_smoothing() {
        let mut input = InputActionMapper::new();
        input.register_axis("walk", [KeyCode::KeyW], [KeyCode::KeyS], 0.5);
        input.input.keys_pressed.insert(KeyCode::KeyW);
        input.input.keys_pressed.insert(KeyCode::KeyS);
        input.tick(1.0 / 60.0);
        assert_eq!(input.get_axis("walk"), 0.0);
    }
}
