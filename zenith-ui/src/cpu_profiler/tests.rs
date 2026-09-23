use super::*;

fn draw(
    ctx: &egui::Context,
    profiler: &mut CpuProfiler,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events,
            ..Default::default()
        },
        |ui| profiler.show(ui),
    );
    output.textures_delta.clear();
    output
}

#[test]
fn capture_button_displays_and_replaces_a_frame_in_both_backend_views() {
    zenith_core::profile::initialize().unwrap();
    let ctx = egui::Context::default();
    let mut profiler = CpuProfiler::default();
    let mut previous = None;
    for view in [puffin_egui::View::Flamegraph, puffin_egui::View::Stats] {
        profiler.viewer.profiler_ui.view = view;
        let output = draw(&ctx, &mut profiler, vec![]);
        let at = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Capture next frame" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut profiler,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        assert!(cpu::capture_pending());
        {
            let _frame = cpu::begin_frame().unwrap();
            zenith_core::profile::scope!("UI test frame");
            draw(&ctx, &mut profiler, vec![]);
        }
        let output = draw(&ctx, &mut profiler, vec![]);
        assert!(profiler.displayed_frame.is_some());
        assert_ne!(profiler.displayed_frame, previous);
        previous = profiler.displayed_frame;
        assert_eq!(
            profiler
                .viewer
                .global_frame_view()
                .lock()
                .all_uniq()
                .count(),
            1
        );
        assert!(!cpu::capture_pending());
        let meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
        assert!(!meshes.is_empty());
        for mesh in meshes {
            if let egui::epaint::Primitive::Mesh(mesh) = mesh.primitive {
                assert!(mesh.vertices.iter().all(|vertex| vertex.pos.is_finite()));
            }
        }
    }
}
