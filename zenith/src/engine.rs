use crate::{RenderContext, RenderableApp};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::window::Window;
use zenith_core::log;
use zenith_rendergraph::{GraphTimings, RenderGraphBuilder, ResourceCache};
use zenith_rhi::{Descriptors, Gpu, Submission, Swapchain};

#[derive(Default)]
struct FrameSlot {
    submission: Option<Submission>,
    timings: Option<PendingTimings>,
    cache: ResourceCache,
}

struct PendingTimings {
    frame_number: u64,
    captured_at: Instant,
    cpu_ms: f64,
    gpu: Option<GraphTimings>,
}

struct CompletedTimings {
    frame_number: u64,
    captured_at: Instant,
    cpu_ms: f64,
    passes: Option<Vec<(String, f64)>>,
}

impl FrameSlot {
    fn finish(
        &mut self,
        totals: &mut BTreeMap<String, (f64, u64)>,
        completed: &mut Vec<CompletedTimings>,
    ) -> anyhow::Result<()> {
        profiling::scope!("Complete previous submission");
        if let Some(mut submission) = self.submission.take() {
            submission.wait(10_000_000_000)?;
        }
        if let Some(timings) = self.timings.take() {
            let passes = timings.gpu.map(|gpu| gpu.read()).transpose()?;
            for (name, ms) in passes.iter().flatten() {
                let total = totals.entry(name.clone()).or_default();
                total.0 += ms;
                total.1 += 1;
            }
            completed.push(CompletedTimings {
                frame_number: timings.frame_number,
                captured_at: timings.captured_at,
                cpu_ms: timings.cpu_ms,
                passes,
            });
        }
        Ok(())
    }
}

pub struct Engine {
    frames: [FrameSlot; 3],
    frame_index: usize,
    swapchain: Swapchain,
    pub gpu: Arc<Gpu>,
    pub descriptors: Arc<Descriptors>,
    pub main_window: Arc<Window>,
    should_exit: bool,
    record_time: f64,
    peak_warm_record: f64,
    rendered: u64,
    profile: bool,
    first_record: f64,
    peak_pipelines: usize,
    allocation_range: (u32, u32),
    gpu_times: BTreeMap<String, (f64, u64)>,
    completed_timings: Vec<CompletedTimings>,
}

impl Engine {
    pub fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let validation = cfg!(debug_assertions) || std::env::var_os("ZENITH_VALIDATION").is_some();
        let swapchain = Swapchain::new(window.clone(), validation)?;
        let gpu = swapchain.gpu().clone();
        let descriptors = Descriptors::new(&gpu, 16384, 256)?;
        Ok(Self {
            frames: std::array::from_fn(|_| FrameSlot::default()),
            frame_index: 0,
            swapchain,
            gpu,
            descriptors,
            main_window: window,
            should_exit: false,
            record_time: 0.0,
            peak_warm_record: 0.0,
            rendered: 0,
            profile: std::env::var_os("ZENITH_PROFILE").is_some(),
            first_record: 0.0,
            peak_pipelines: 0,
            allocation_range: (u32::MAX, 0),
            gpu_times: BTreeMap::new(),
            completed_timings: Vec::new(),
        })
    }
    pub fn tick(&mut self, _delta_time: f32) {}
    pub fn render<A: RenderableApp>(&mut self, app: &mut A) -> anyhow::Result<bool> {
        self.render_with_update_time(app, Duration::ZERO)
    }
    pub(crate) fn render_with_update_time<A: RenderableApp>(
        &mut self,
        app: &mut A,
        update_time: Duration,
    ) -> anyhow::Result<bool> {
        profiling::scope!("Render frame");
        let slot = &mut self.frames[self.frame_index];
        slot.finish(&mut self.gpu_times, &mut self.completed_timings)?;
        self.completed_timings
            .sort_unstable_by_key(|frame| frame.frame_number);
        for frame in self.completed_timings.drain(..) {
            let gpu_ms = frame
                .passes
                .as_ref()
                .map(|passes| passes.iter().map(|(_, ms)| ms).sum());
            app.on_frame_timings(frame.frame_number, frame.cpu_ms, gpu_ms);
            if let Some(passes) = frame.passes {
                app.on_gpu_timings(frame.frame_number, frame.captured_at, &passes);
            }
        }
        let Some(frame) = self.swapchain.acquire()? else {
            return Ok(false);
        };
        let start = Instant::now();
        let mut graph = RenderGraphBuilder::new(&self.gpu, &self.descriptors, &mut slot.cache)?;
        let output = graph.import_frame(frame.texture().clone())?;
        {
            profiling::scope!("App render graph build");
            app.render(
                &mut graph,
                RenderContext {
                    output,
                    extent: self.swapchain.extent(),
                    frame_index: self.frame_index,
                },
            )?;
        }
        let (commands, timings) =
            graph.record_profiled(self.profile || app.gpu_timing_enabled())?;
        let elapsed = start.elapsed().as_secs_f64();
        self.record_time += elapsed;
        if self.rendered == 0 {
            self.first_record = elapsed;
        } else {
            self.peak_warm_record = self.peak_warm_record.max(elapsed);
        }
        slot.submission = Some(frame.present(commands)?);
        slot.timings = Some(PendingTimings {
            frame_number: self.rendered + 1,
            captured_at: Instant::now(),
            cpu_ms: (update_time.as_secs_f64() + elapsed) * 1000.0,
            gpu: timings,
        });
        if zenith_core::profile::startup_needs_completion() {
            slot.submission.as_mut().unwrap().wait(10_000_000_000)?;
            zenith_core::profile::complete_startup();
        }
        self.rendered += 1;
        if let Some(count) = self.gpu.try_pipeline_count() {
            self.peak_pipelines = self.peak_pipelines.max(count);
        }
        if self.rendered >= 6 {
            let allocations = self.gpu.allocation_count()?;
            self.allocation_range.0 = self.allocation_range.0.min(allocations);
            self.allocation_range.1 = self.allocation_range.1.max(allocations);
        }
        self.frame_index = (self.frame_index + 1) % self.frames.len();
        Ok(true)
    }
    pub fn resize(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        for slot in &mut self.frames {
            slot.finish(&mut self.gpu_times, &mut self.completed_timings)?;
            slot.cache.clear();
        }
        self.swapchain.resize()
    }
    pub fn request_exit(&mut self) {
        self.should_exit = true;
    }
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }
    pub fn pipeline_cache_size(&self) -> usize {
        self.gpu.try_pipeline_count().unwrap_or(self.peak_pipelines)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for slot in &mut self.frames {
            if let Err(error) = slot.finish(&mut self.gpu_times, &mut self.completed_timings) {
                log::error!("frame completion failed: {error:#}");
            }
        }
        if self.rendered > 0 {
            log::info!("Rendered {} frames; CPU graph build/record cold {:.3} ms, warm mean {:.3} ms, warm peak {:.3} ms; peak pipelines {}; steady VMA allocations {:?}; descriptor writes {:?}", self.rendered, self.first_record * 1000.0, (self.record_time - self.first_record) * 1000.0 / self.rendered.saturating_sub(1).max(1) as f64, self.peak_warm_record * 1000.0, self.peak_pipelines, self.allocation_range, self.descriptors.write_counts());
            for (name, (total, count)) in &self.gpu_times {
                log::info!(
                    "GPU {name}: {:.3} ms mean ({count} samples)",
                    total / *count as f64
                );
            }
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests;
