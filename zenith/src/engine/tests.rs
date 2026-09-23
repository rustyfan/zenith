use super::*;
use crate::{App, Args};
use winit::{event_loop::EventLoop, platform::windows::EventLoopBuilderExtWindows};
use zenith_rhi::Access;

#[derive(Default)]
struct TimingApp {
    enabled: bool,
    frames: Vec<(u64, Instant)>,
    totals: Vec<(u64, f64, Option<f64>)>,
}

impl App for TimingApp {
    fn new(_: &Args) -> anyhow::Result<Self> {
        Ok(Self::default())
    }
}

impl RenderableApp for TimingApp {
    fn gpu_timing_enabled(&self) -> bool {
        self.enabled
    }

    fn on_gpu_timings(&mut self, number: u64, captured_at: Instant, passes: &[(String, f64)]) {
        assert_eq!(passes.len(), 2);
        assert!(passes
            .iter()
            .all(|(name, ms)| name == "clear" && ms.is_finite() && *ms >= 0.0));
        let &(total_number, _, gpu_ms) = self.totals.last().unwrap();
        assert_eq!(total_number, number);
        assert_eq!(gpu_ms, Some(passes.iter().map(|(_, ms)| ms).sum()));
        self.frames.push((number, captured_at));
    }

    fn on_frame_timings(&mut self, number: u64, cpu_ms: f64, gpu_ms: Option<f64>) {
        assert!(cpu_ms.is_finite() && cpu_ms >= 2.0);
        assert!(gpu_ms.is_none_or(|ms| ms.is_finite() && ms >= 0.0));
        self.totals.push((number, cpu_ms, gpu_ms));
    }

    fn render(
        &mut self,
        graph: &mut RenderGraphBuilder<'_>,
        context: RenderContext,
    ) -> anyhow::Result<()> {
        for _ in 0..2 {
            graph.pass(
                "clear",
                vec![context.output.write(Access::COPY_WRITE)],
                move |ctx| {
                    ctx.commands
                        .clear_color(&ctx.image(context.output)?, [0.0, 0.0, 0.0, 1.0])
                },
            )?;
        }
        Ok(())
    }
}

#[test]
#[ignore = "requires Windows and Vulkan validation"]
fn timings_follow_completed_frames_across_resize_and_runtime_toggles() -> anyhow::Result<()> {
    let _ = zenith_core::log::initialize(zenith_core::log::LevelFilter::Info);
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    #[allow(deprecated)]
    let window = Arc::new(
        event_loop.create_window(
            Window::default_attributes()
                .with_visible(false)
                .with_inner_size(winit::dpi::PhysicalSize::new(64, 64)),
        )?,
    );
    let mut engine = Engine::new(window)?;
    engine.profile = false;
    let instance = engine.gpu.instance().clone();
    anyhow::ensure!(
        instance.validation_enabled(),
        "timing test requires Vulkan validation"
    );
    let mut app = TimingApp::default();
    for _ in 0..4 {
        assert!(engine.render_with_update_time(&mut app, Duration::from_millis(2))?);
    }
    assert!(app.frames.is_empty());
    assert!(engine
        .frames
        .iter()
        .all(|slot| slot.timings.as_ref().unwrap().gpu.is_none()));
    assert_eq!(app.totals.len(), 1);
    assert_eq!(app.totals[0].0, 1);
    assert_eq!(app.totals[0].2, None);
    app.enabled = true;
    for _ in 0..5 {
        assert!(engine.render_with_update_time(&mut app, Duration::from_millis(2))?);
    }
    assert_eq!(
        app.frames.iter().map(|frame| frame.0).collect::<Vec<_>>(),
        [5, 6]
    );
    engine.resize(64, 64)?;
    app.enabled = false;
    for _ in 0..4 {
        assert!(engine.render_with_update_time(&mut app, Duration::from_millis(2))?);
    }
    assert_eq!(
        app.frames.iter().map(|frame| frame.0).collect::<Vec<_>>(),
        [5, 6, 7, 8, 9]
    );
    assert!(engine
        .frames
        .iter()
        .all(|slot| slot.timings.as_ref().unwrap().gpu.is_none()));
    app.enabled = true;
    for _ in 0..3 {
        assert!(engine.render_with_update_time(&mut app, Duration::from_millis(2))?);
    }
    app.enabled = false;
    for _ in 0..4 {
        assert!(engine.render_with_update_time(&mut app, Duration::from_millis(2))?);
    }
    assert_eq!(
        app.frames.iter().map(|frame| frame.0).collect::<Vec<_>>(),
        [5, 6, 7, 8, 9, 14, 15, 16]
    );
    assert!(app
        .frames
        .windows(2)
        .all(|frames| frames[0].1 <= frames[1].1));
    engine.profile = true;
    for _ in 0..4 {
        assert!(engine.render_with_update_time(&mut app, Duration::from_millis(2))?);
    }
    assert_eq!(app.frames.last().unwrap().0, 21);
    assert_eq!(
        app.totals.iter().map(|frame| frame.0).collect::<Vec<_>>(),
        (1..=21).collect::<Vec<_>>()
    );
    assert_eq!(
        app.totals
            .iter()
            .filter(|frame| frame.2.is_some())
            .map(|frame| frame.0)
            .collect::<Vec<_>>(),
        app.frames.iter().map(|frame| frame.0).collect::<Vec<_>>()
    );
    drop(engine);
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "GPU timing validation: {:?}",
        instance.validation_errors()
    );
    Ok(())
}
