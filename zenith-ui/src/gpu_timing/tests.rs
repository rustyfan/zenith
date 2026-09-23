use super::*;

fn sample(graph: &mut GpuTimingGraph, number: u64, at: Instant, passes: &[(&str, f64)]) {
    graph.push_frame(
        number,
        at,
        &passes
            .iter()
            .map(|(name, ms)| (name.to_string(), *ms))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn window_uses_capture_time_and_history_has_a_hard_bound() {
    let mut graph = GpuTimingGraph::default();
    let start = Instant::now();
    assert_eq!(graph.window_seconds(), 1.0);
    for (number, ms) in [(1, 0), (2, 10), (3, 500), (4, 1500)] {
        sample(
            &mut graph,
            number,
            start + Duration::from_millis(ms),
            &[("draw", 2.0)],
        );
    }
    assert_eq!(
        graph.frames.iter().map(|f| f.number).collect::<Vec<_>>(),
        [3, 4]
    );
    graph.set_window_seconds(0.25);
    assert_eq!(graph.frames.len(), 1);
    graph.set_window_seconds(f32::NAN);
    assert_eq!(graph.window_seconds(), 0.25);
    graph.clear();
    for number in 0..MAX_FRAMES as u64 + 10 {
        sample(&mut graph, number, start, &[("draw", 1.0)]);
    }
    assert_eq!(graph.frames.len(), MAX_FRAMES);
    assert_eq!(graph.frames.front().unwrap().number, 10);
}

#[test]
fn pause_freezes_history_and_stale_frames_cannot_reorder_it() {
    let mut graph = GpuTimingGraph::default();
    let start = Instant::now();
    sample(&mut graph, 10, start, &[("draw", 2.0)]);
    graph.set_paused(true);
    sample(
        &mut graph,
        11,
        start + Duration::from_secs(10),
        &[("draw", 9.0)],
    );
    assert_eq!(graph.frames.back().unwrap().number, 10);
    graph.set_paused(false);
    sample(
        &mut graph,
        12,
        start + Duration::from_secs(11),
        &[("draw", 3.0)],
    );
    sample(
        &mut graph,
        11,
        start + Duration::from_secs(12),
        &[("draw", 9.0)],
    );
    sample(&mut graph, 13, start, &[("draw", 9.0)]);
    assert_eq!(graph.frames.len(), 1);
    assert_eq!(graph.frames.back().unwrap().number, 12);
    graph.clear();
    assert!(graph.frames.is_empty() && graph.passes.is_empty());
    sample(&mut graph, 1, start, &[("draw", 1.0)]);
    assert_eq!(graph.frames.len(), 1);
}

#[test]
fn changing_passes_preserves_identity_color_and_separate_occurrences() {
    let mut graph = GpuTimingGraph::default();
    let start = Instant::now();
    sample(
        &mut graph,
        1,
        start,
        &[("upload", 1.0), ("upload", 2.0), ("draw", 3.0)],
    );
    let colors = graph
        .passes
        .iter()
        .map(|(id, pass)| (id.clone(), pass.color))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(colors.len(), 3);
    let upload = PassId {
        name: "upload".into(),
        occurrence: 1,
    };
    let second = PassId {
        name: "upload".into(),
        occurrence: 2,
    };
    assert_eq!(second.label(), "upload #2");
    assert_ne!(colors[&upload], colors[&second]);
    sample(
        &mut graph,
        2,
        start,
        &[
            ("draw", 4.0),
            ("upload", f64::NAN),
            ("upload", 5.0),
            ("bad", -1.0),
        ],
    );
    let frame = graph.frames.back().unwrap();
    assert!(!frame.passes.contains_key(&upload));
    assert_eq!(frame.passes[&second], 5.0);
    for (id, color) in &colors {
        assert_eq!(graph.passes[id].color, *color);
    }
    graph.passes.get_mut(&second).unwrap().visible = false;
    assert!(graph.max_ms() < 5.0);
    sample(
        &mut graph,
        3,
        start + Duration::from_secs(2),
        &[("draw", 1.0)],
    );
    assert_eq!(graph.passes.len(), 1);
    sample(
        &mut graph,
        4,
        start + Duration::from_secs(2),
        &[("upload", 1.0)],
    );
    assert_eq!(graph.passes[&upload].color, colors[&upload]);
}

#[test]
fn random_palettes_stay_bright_distinct_and_stable() {
    for color_seed in 0..64 {
        let mut graph = GpuTimingGraph {
            color_seed,
            ..Default::default()
        };
        let start = Instant::now();
        let timings = (0..24)
            .map(|index| (format!("pass_{index}"), 1.0))
            .collect::<Vec<_>>();
        graph.push_frame(1, start, &timings);
        let colors = timings
            .iter()
            .map(|(name, _)| {
                graph.colors[&PassId {
                    name: name.clone(),
                    occurrence: 1,
                }]
            })
            .collect::<Vec<_>>();
        for (index, &color) in colors.iter().enumerate() {
            let lab = oklab(color);
            assert!(lab[0] >= 0.65);
            for &other in &colors[..index] {
                let distance = color_distance(lab, oklab(other)).sqrt();
                let minimum = if index < 12 { 0.10 } else { 0.075 };
                assert!(
                    distance >= minimum,
                    "passes {index}: color distance {distance} < {minimum}"
                );
            }
        }
        let assigned = graph.colors.clone();
        graph.clear();
        let mut reordered = timings;
        reordered.reverse();
        graph.push_frame(2, start, &reordered);
        assert!(graph
            .passes
            .iter()
            .all(|(id, pass)| pass.color == assigned[id]));
    }
}

fn draw(
    ctx: &egui::Context,
    graph: &mut GpuTimingGraph,
    events: Vec<egui::Event>,
    window: bool,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0))),
            events,
            ..Default::default()
        },
        |root| {
            if window {
                egui::Window::new("Host")
                    .fade_in(false)
                    .default_width(600.0)
                    .show(root, |ui| {
                        graph.show(ui);
                    });
            } else {
                root.set_max_width(260.0);
                egui::CollapsingHeader::new("Embedded")
                    .default_open(true)
                    .show(root, |ui| {
                        graph.show(ui);
                    });
            }
        },
    );
    output.textures_delta.clear();
    output
}

fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing text: {label}"))
}

fn click(ctx: &egui::Context, graph: &mut GpuTimingGraph, at: egui::Pos2) -> egui::FullOutput {
    draw(
        ctx,
        graph,
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
        true,
    );
    draw(
        ctx,
        graph,
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
        true,
    )
}

#[test]
fn widget_rehosts_and_controls_pause_visibility_and_clear() {
    let ctx = egui::Context::default();
    let mut graph = GpuTimingGraph::default();
    sample(&mut graph, 1, Instant::now(), &[("draw", 2.0)]);
    for window in [false, true, false, true] {
        draw(&ctx, &mut graph, Vec::new(), window);
        for _ in 0..2 {
            let output = draw(&ctx, &mut graph, Vec::new(), window);
            let meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
            assert!(!meshes.is_empty());
            for mesh in meshes {
                if let egui::epaint::Primitive::Mesh(mesh) = mesh.primitive {
                    assert!(mesh.vertices.iter().all(|vertex| vertex.pos.is_finite()));
                }
            }
        }
        assert_eq!(graph.frames.len(), 1);
    }
    let output = draw(&ctx, &mut graph, Vec::new(), true);
    let at = text_position(&output, "Pause");
    let output = click(&ctx, &mut graph, at);
    assert!(graph.is_paused());
    let at = text_position(&output, "draw");
    let output = click(&ctx, &mut graph, at);
    assert!(!graph.passes.values().next().unwrap().visible);
    let at = text_position(&output, "Pause");
    let output = click(&ctx, &mut graph, at);
    assert!(!graph.is_paused());
    let at = text_position(&output, "1.00");
    click(&ctx, &mut graph, at);
    let output = draw(
        &ctx,
        &mut graph,
        vec![
            egui::Event::Text("2.5".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            },
        ],
        true,
    );
    assert_eq!(graph.window_seconds(), 2.5);
    let at = text_position(&output, "Clear");
    click(&ctx, &mut graph, at);
    assert!(graph.frames.is_empty());
}

#[test]
fn plot_leaves_gaps_for_missing_passes_and_unsampled_frames() {
    let ctx = egui::Context::default();
    let mut graph = GpuTimingGraph::default();
    let start = Instant::now();
    for number in [1, 2, 3, 4, 5, 8] {
        sample(
            &mut graph,
            number,
            start,
            if number == 3 { &[] } else { &[("draw", 2.0)] },
        );
    }
    let color = graph.passes.values().next().unwrap().color;
    draw(&ctx, &mut graph, Vec::new(), true);
    let output = draw(&ctx, &mut graph, Vec::new(), true);
    let lines = output
        .shapes
        .iter()
        .filter(|shape| {
            matches!(shape.shape,
                egui::Shape::LineSegment { stroke, .. } if stroke.color == color
            )
        })
        .count();
    assert_eq!(lines, 2);
}
