use egui::{pos2, vec2, Pos2, Rect, Sense, Ui};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    window::{ResizeDirection, Window},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelRequest {
    Detach(Rect),
    Dock,
}

struct Drag {
    pointer: Pos2,
    position: Pos2,
    moved: bool,
}

struct ResizeDrag {
    pointer: Pos2,
    bounds: Rect,
    direction: ResizeDirection,
}

impl ResizeDrag {
    fn bounds_at(&self, pointer: Pos2, min_size: egui::Vec2) -> Rect {
        use ResizeDirection::*;
        let delta = pointer - self.pointer;
        let mut bounds = self.bounds;
        if matches!(self.direction, West | NorthWest | SouthWest) {
            bounds.min.x = (bounds.min.x + delta.x).min(bounds.max.x - min_size.x);
        }
        if matches!(self.direction, East | NorthEast | SouthEast) {
            bounds.max.x = (bounds.max.x + delta.x).max(bounds.min.x + min_size.x);
        }
        if matches!(self.direction, North | NorthWest | NorthEast) {
            bounds.min.y = (bounds.min.y + delta.y).min(bounds.max.y - min_size.y);
        }
        if matches!(self.direction, South | SouthWest | SouthEast) {
            bounds.max.y = (bounds.max.y + delta.y).max(bounds.min.y + min_size.y);
        }
        bounds
    }
}

pub struct DetachablePanel {
    title: String,
    rect: Rect,
    header: Rect,
    pointer: Option<Pos2>,
    drag: Option<Drag>,
    resize: Option<ResizeDrag>,
    request: Option<PanelRequest>,
    was_detached: bool,
}

impl DetachablePanel {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            rect: Rect::from_min_size(pos2(16.0, 16.0), vec2(600.0, 460.0)),
            header: Rect::NOTHING,
            pointer: None,
            drag: None,
            resize: None,
            request: None,
            was_detached: false,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn take_request(&mut self) -> Option<PanelRequest> {
        self.request.take()
    }

    pub fn request_dock(&mut self) {
        self.request = Some(PanelRequest::Dock);
    }

    pub fn on_window_event(
        &mut self,
        event: &WindowEvent,
        window: &Window,
        pixels_per_point: f32,
    ) -> bool {
        let bounds = Rect::from_min_size(
            Pos2::ZERO,
            vec2(
                window.inner_size().width as f32,
                window.inner_size().height as f32,
            ) / pixels_per_point,
        );
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.move_pointer(pos2(position.x as f32, position.y as f32) / pixels_per_point);
                self.drag.is_some()
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if *state == ElementState::Pressed {
                    self.press_pointer();
                    self.drag.is_some()
                } else {
                    let dragging = self.drag.is_some();
                    self.release_pointer(bounds);
                    dragging
                }
            }
            WindowEvent::Focused(false) => {
                if let Some(drag) = self.drag.take() {
                    self.rect = Rect::from_min_size(drag.position, self.rect.size());
                }
                self.pointer = None;
                false
            }
            WindowEvent::CursorLeft { .. } if self.drag.is_none() => {
                self.pointer = None;
                false
            }
            _ => false,
        }
    }

    pub fn on_detached_window_event(
        &mut self,
        event: &WindowEvent,
        window: &Window,
        pixels_per_point: f32,
    ) -> bool {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = Some(pos2(position.x as f32, position.y as f32) / pixels_per_point);
                if let Some(resize) = &self.resize {
                    if let Ok(origin) = window.inner_position() {
                        let pointer = pos2(origin.x as f32, origin.y as f32)
                            + vec2(position.x as f32, position.y as f32);
                        let bounds = resize
                            .bounds_at(pointer, vec2(280.0, 200.0) * window.scale_factor() as f32);
                        let _ = window.request_inner_size(PhysicalSize::new(
                            bounds.width().round() as u32,
                            bounds.height().round() as u32,
                        ));
                        window.set_outer_position(PhysicalPosition::new(
                            bounds.min.x.round() as i32,
                            bounds.min.y.round() as i32,
                        ));
                    }
                    return true;
                }
            }
            WindowEvent::Focused(false) => {
                self.resize = None;
                self.pointer = None;
            }
            WindowEvent::CursorLeft { .. } if self.resize.is_none() => {
                self.pointer = None;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(pointer) = self.pointer {
                    let size = window.inner_size();
                    let bounds = Rect::from_min_size(
                        Pos2::ZERO,
                        vec2(size.width as f32, size.height as f32) / pixels_per_point,
                    );
                    if let Some(direction) = resize_direction(pointer, bounds) {
                        if let (Ok(inner), Ok(outer)) =
                            (window.inner_position(), window.outer_position())
                        {
                            self.resize = Some(ResizeDrag {
                                pointer: pos2(inner.x as f32, inner.y as f32)
                                    + pointer.to_vec2() * pixels_per_point,
                                bounds: Rect::from_min_size(
                                    pos2(outer.x as f32, outer.y as f32),
                                    vec2(size.width as f32, size.height as f32),
                                ),
                                direction,
                            });
                            return true;
                        }
                    }
                    if self.header.contains(pointer) {
                        return window.drag_window().is_ok();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => return self.resize.take().is_some(),
            _ => {}
        }
        false
    }

    fn move_pointer(&mut self, pointer: Pos2) {
        self.pointer = Some(pointer);
        if let Some(drag) = &mut self.drag {
            let delta = pointer - drag.pointer;
            drag.moved |= delta.length() >= 6.0;
            if drag.moved {
                self.rect = Rect::from_min_size(drag.position + delta, self.rect.size());
            }
        }
    }

    fn press_pointer(&mut self) {
        if let Some(pointer) = self
            .pointer
            .filter(|pointer| self.header.contains(*pointer))
        {
            self.drag = Some(Drag {
                pointer,
                position: self.rect.min,
                moved: false,
            });
        }
    }

    fn release_pointer(&mut self, bounds: Rect) {
        if let Some(drag) = self.drag.take() {
            if drag.moved
                && self
                    .pointer
                    .is_some_and(|pointer| !bounds.expand(8.0).contains(pointer))
            {
                self.request = Some(PanelRequest::Detach(self.rect));
                self.rect = Rect::from_min_size(drag.position, self.rect.size());
            } else {
                self.rect = Rect::from_min_size(
                    self.rect
                        .min
                        .clamp(bounds.min, (bounds.max - vec2(100.0, 30.0)).max(bounds.min)),
                    self.rect.size(),
                );
            }
        }
    }

    pub fn show(&mut self, root: &mut Ui, detached: bool, build: impl FnOnce(&mut Ui)) {
        if self.was_detached != detached {
            self.pointer = None;
            self.drag = None;
            self.resize = None;
        }
        let mut window = egui::Window::new(&self.title)
            .title_bar(false)
            .movable(false)
            .constrain(false)
            .current_pos(self.rect.min)
            .default_size(self.rect.size())
            .min_width(280.0);
        if detached {
            window = window.fixed_rect(root.max_rect()).resizable(false);
        } else if self.was_detached {
            window = window.fixed_size(self.rect.size()).resizable(false);
        }
        self.was_detached = detached;
        let response = window.show(root, |ui| {
            ui.horizontal(|ui| {
                let header = ui.add_sized(
                    vec2((ui.available_width() - 90.0).max(80.0), 24.0),
                    egui::Label::new(egui::RichText::new(&self.title).strong())
                        .sense(Sense::drag()),
                );
                self.header = header.rect;
                if detached {
                    header.on_hover_cursor(egui::CursorIcon::Grab);
                } else {
                    header.on_hover_text("Drag this title outside the app and release to detach.");
                }
                if ui
                    .button(if detached { "Dock back" } else { "Detach" })
                    .clicked()
                {
                    self.request = Some(if detached {
                        PanelRequest::Dock
                    } else {
                        PanelRequest::Detach(self.rect)
                    });
                }
            });
            ui.separator();
            build(ui);
        });
        if !detached {
            if let Some(response) = response {
                self.rect = response.response.rect;
            }
        } else if let Some(direction) = root
            .ctx()
            .pointer_hover_pos()
            .and_then(|pointer| resize_direction(pointer, root.max_rect()))
        {
            use ResizeDirection::*;
            root.ctx().set_cursor_icon(match direction {
                East | West => egui::CursorIcon::ResizeHorizontal,
                North | South => egui::CursorIcon::ResizeVertical,
                NorthEast | SouthWest => egui::CursorIcon::ResizeNeSw,
                NorthWest | SouthEast => egui::CursorIcon::ResizeNwSe,
            });
        }
    }
}

fn resize_direction(pointer: Pos2, bounds: Rect) -> Option<ResizeDirection> {
    if !bounds.contains(pointer) {
        return None;
    }
    let left = pointer.x < bounds.left() + 6.0;
    let right = pointer.x > bounds.right() - 6.0;
    let top = pointer.y < bounds.top() + 6.0;
    let bottom = pointer.y > bounds.bottom() - 6.0;
    use ResizeDirection::*;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(NorthWest),
        (_, true, true, _) => Some(NorthEast),
        (true, _, _, true) => Some(SouthWest),
        (_, true, _, true) => Some(SouthEast),
        (true, _, _, _) => Some(West),
        (_, true, _, _) => Some(East),
        (_, _, true, _) => Some(North),
        (_, _, _, true) => Some(South),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
