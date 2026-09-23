use super::*;

fn panel() -> DetachablePanel {
    let mut panel = DetachablePanel::new("Tools");
    panel.header = Rect::from_min_size(pos2(24.0, 24.0), vec2(480.0, 24.0));
    panel
}

#[test]
fn borderless_resize_hits_all_edges_and_corners_but_not_controls() {
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 460.0));
    use ResizeDirection::*;
    for (pointer, direction) in [
        (pos2(2.0, 2.0), NorthWest),
        (pos2(598.0, 2.0), NorthEast),
        (pos2(2.0, 458.0), SouthWest),
        (pos2(598.0, 458.0), SouthEast),
        (pos2(2.0, 230.0), West),
        (pos2(598.0, 230.0), East),
        (pos2(300.0, 2.0), North),
        (pos2(300.0, 458.0), South),
    ] {
        assert_eq!(resize_direction(pointer, bounds), Some(direction));
    }
    for pointer in [pos2(100.0, 20.0), pos2(550.0, 20.0), pos2(-1.0, 50.0)] {
        assert_eq!(resize_direction(pointer, bounds), None);
    }
}

#[test]
fn resizing_preserves_opposite_edges_and_enforces_minimum_size() {
    let bounds = Rect::from_min_size(pos2(-600.0, 100.0), vec2(900.0, 690.0));
    let minimum = vec2(420.0, 300.0);
    let drag = ResizeDrag {
        pointer: bounds.min,
        bounds,
        direction: ResizeDirection::NorthWest,
    };
    let resized = drag.bounds_at(bounds.max, minimum);
    assert_eq!(resized.max, bounds.max);
    assert_eq!(resized.size(), minimum);
    let drag = ResizeDrag {
        pointer: bounds.max,
        bounds,
        direction: ResizeDirection::SouthEast,
    };
    let resized = drag.bounds_at(bounds.max + vec2(100.0, 50.0), minimum);
    assert_eq!(resized.min, bounds.min);
    assert_eq!(resized.size(), vec2(1000.0, 740.0));
}

#[test]
fn only_title_drags_released_outside_detach_once() {
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
    let mut panel = panel();
    panel.move_pointer(pos2(100.0, 100.0));
    panel.press_pointer();
    panel.move_pointer(pos2(900.0, 100.0));
    panel.release_pointer(bounds);
    assert!(panel.take_request().is_none());
    panel.move_pointer(pos2(100.0, 30.0));
    panel.press_pointer();
    panel.move_pointer(pos2(900.0, 130.0));
    panel.release_pointer(bounds);
    let Some(PanelRequest::Detach(rect)) = panel.take_request() else {
        panic!("title drag should detach");
    };
    assert_eq!(rect.min, pos2(816.0, 116.0));
    assert_eq!(rect.size(), vec2(600.0, 460.0));
    assert_eq!(panel.rect.min, pos2(16.0, 16.0));
    assert!(panel.take_request().is_none());
}

#[test]
fn inside_drop_and_boundary_jitter_stay_embedded() {
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
    for destination in [pos2(200.0, 50.0), pos2(-4.0, 50.0), pos2(804.0, 50.0)] {
        let mut panel = panel();
        panel.move_pointer(pos2(100.0, 30.0));
        panel.press_pointer();
        panel.move_pointer(destination);
        panel.release_pointer(bounds);
        assert!(panel.take_request().is_none());
        assert!(bounds.contains(panel.rect.min));
    }
    for destination in [
        pos2(-10.0, 50.0),
        pos2(810.0, 50.0),
        pos2(100.0, -10.0),
        pos2(100.0, 610.0),
    ] {
        let mut panel = panel();
        panel.move_pointer(pos2(100.0, 30.0));
        panel.press_pointer();
        panel.move_pointer(destination);
        panel.release_pointer(bounds);
        assert!(matches!(
            panel.take_request(),
            Some(PanelRequest::Detach(_))
        ));
    }
}

#[test]
fn rehosting_preserves_widget_ids_and_embedded_placement() {
    let context = egui::Context::default();
    let mut panel = panel();
    let mut ids = Vec::new();
    let mut embedded_size = None;
    for (index, detached) in [false, false, true, false, false].into_iter().enumerate() {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))),
                ..Default::default()
            },
            |root| {
                panel.show(root, detached, |ui| {
                    ids.push(ui.id());
                })
            },
        );
        output.textures_delta.clear();
        if index > 0 && !detached {
            if let Some(size) = embedded_size {
                assert_eq!(panel.rect.size(), size);
            } else {
                embedded_size = Some(panel.rect.size());
            }
        }
    }
    assert!(ids.windows(2).all(|ids| ids[0] == ids[1]));
    assert_eq!(panel.rect.min, pos2(16.0, 16.0));
    panel.request_dock();
    assert_eq!(panel.take_request(), Some(PanelRequest::Dock));
}

#[test]
fn buttons_request_detach_and_dock_without_changing_content() {
    let context = egui::Context::default();
    let mut panel = panel();
    let mut draw = |detached, events| {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |root| {
                panel.show(root, detached, |ui| {
                    ui.label("Persistent content");
                })
            },
        );
        output.textures_delta.clear();
        (output, panel.take_request())
    };
    for (detached, label) in [(false, "Detach"), (true, "Dock back")] {
        draw(detached, Vec::new());
        let (output, _) = draw(detached, Vec::new());
        let at = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("host action button");
        let event = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        draw(detached, vec![egui::Event::PointerMoved(at), event(true)]);
        let (_, request) = draw(detached, vec![event(false)]);
        if detached {
            assert_eq!(request, Some(PanelRequest::Dock));
        } else {
            assert!(matches!(request, Some(PanelRequest::Detach(_))));
        }
    }
}
