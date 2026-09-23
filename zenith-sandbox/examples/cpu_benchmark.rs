#[allow(dead_code)]
#[path = "world.rs"]
mod world;

#[cfg(feature = "benchmark-allocations")]
#[path = "support/allocation_counter.rs"]
mod allocation_counter;
#[cfg(feature = "cpu-profiling")]
#[path = "support/cpu_scope_report.rs"]
mod cpu_scope_report;

use std::{io::Write, sync::Arc, time::Instant};
use winit::{event::WindowEvent, window::Window};
use zenith::rendergraph::RenderGraphBuilder;
use zenith::rhi::{Descriptors, Gpu};
use zenith::{App, Args, RenderContext, RenderableApp};

struct Benchmark {
    scene: world::WorldApp,
    warmup: u64,
    frames: u64,
    samples: Vec<(u64, f64, f64)>,
    wall: Vec<(f64, u64, u64)>,
    previous_tick: Instant,
    #[cfg(feature = "cpu-profiling")]
    scopes: cpu_scope_report::Report,
}

fn setting(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .map(|s| s.parse().expect("invalid benchmark setting"))
        .unwrap_or(default)
}

impl App for Benchmark {
    fn new(args: &Args) -> anyhow::Result<Self> {
        Ok(Self {
            scene: world::WorldApp::new(args)?,
            warmup: setting("ZENITH_BENCH_WARMUP", 3000),
            frames: 0,
            samples: Vec::with_capacity(20000),
            wall: Vec::with_capacity(20000),
            previous_tick: Instant::now(),
            #[cfg(feature = "cpu-profiling")]
            scopes: Default::default(),
        })
    }

    fn on_window_event(&mut self, event: &WindowEvent, window: &Window) {
        self.scene.on_window_event(event, window);
    }

    fn tick(&mut self, delta: f32) {
        let now = Instant::now();
        #[cfg(feature = "benchmark-allocations")]
        let (allocations, bytes) = allocation_counter::take();
        #[cfg(not(feature = "benchmark-allocations"))]
        let (allocations, bytes) = (0, 0);
        self.wall.push((
            (now - self.previous_tick).as_secs_f64() * 1000.0,
            allocations,
            bytes,
        ));
        self.previous_tick = now;
        self.frames += 1;
        #[cfg(feature = "cpu-profiling")]
        if self.frames > self.warmup {
            self.scopes.capture();
        }
        self.scene.tick(delta);
    }
}

impl RenderableApp for Benchmark {
    fn prepare(
        &mut self,
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        window: Arc<Window>,
    ) -> anyhow::Result<()> {
        eprintln!(
            "CPU benchmark: adapter={}; driver={}; size={:?}; validation={}; scopes={}; allocation_counter={}",
            gpu.info.name,
            gpu.info.driver_info,
            window.inner_size(),
            gpu.instance().validation_enabled(),
            cfg!(feature = "cpu-profiling"),
            cfg!(feature = "benchmark-allocations"),
        );
        self.scene.prepare(gpu, descriptors, window)
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.scene.resize(width, height);
    }
    fn gpu_timing_enabled(&self) -> bool {
        true
    }
    fn on_gpu_timings(&mut self, number: u64, at: Instant, passes: &[(String, f64)]) {
        self.scene.on_gpu_timings(number, at, passes);
    }
    fn on_frame_timings(&mut self, number: u64, cpu_ms: f64, gpu_ms: Option<f64>) {
        self.scene.on_frame_timings(number, cpu_ms, gpu_ms);
        if number > self.warmup {
            self.samples
                .push((number, cpu_ms, gpu_ms.expect("GPU timestamps required")));
        }
    }
    fn render(
        &mut self,
        builder: &mut RenderGraphBuilder,
        context: RenderContext,
    ) -> anyhow::Result<()> {
        self.scene.render(builder, context)
    }
}

impl Drop for Benchmark {
    fn drop(&mut self) {
        let path = std::env::var("ZENITH_BENCH_OUTPUT").expect("set ZENITH_BENCH_OUTPUT");
        let mut output = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(
            output,
            "frame,cpu_ms,gpu_ms,wall_ms,allocations,allocated_bytes"
        )
        .unwrap();
        for &(number, cpu_ms, gpu_ms) in &self.samples {
            let (wall_ms, allocations, bytes) = self.wall[number as usize];
            writeln!(
                output,
                "{number},{cpu_ms:.6},{gpu_ms:.6},{wall_ms:.6},{allocations},{bytes}"
            )
            .unwrap();
        }
        #[cfg(feature = "cpu-profiling")]
        self.scopes.write(&format!("{path}.scopes.csv"));
    }
}

fn main() {
    std::env::set_var(
        "ZENITH_TEST_FRAMES",
        (setting("ZENITH_BENCH_WARMUP", 3000) + setting("ZENITH_BENCH_FRAMES", 6000) + 3)
            .to_string(),
    );
    zenith::launch::<Benchmark>().unwrap();
}
