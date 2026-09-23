use anyhow::Result;
use std::{collections::BTreeMap, io::Write, path::Path, sync::Arc};
use zenith::{
    core::{camera::Camera, log},
    renderer::WorldRenderer,
    rendergraph::{RenderGraphBuilder, ResourceCache},
    rhi::{vk, Descriptors, Gpu, TextureDesc},
};

pub fn measure(
    renderer: &mut WorldRenderer,
    gpu: &Arc<Gpu>,
    descriptors: &Arc<Descriptors>,
    camera: &Camera,
    extent: [u32; 2],
    path: &Path,
) -> Result<()> {
    const WARMUP: usize = 32;
    const FRAMES: usize = 128;
    let mut cache = ResourceCache::default();
    let timestamps = gpu.timestamps(2)?;
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    let mut csv = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(csv, "frame,pass,milliseconds")?;
    for frame in 0..WARMUP + FRAMES {
        let mut builder = RenderGraphBuilder::new(gpu, descriptors, &mut cache)?;
        let start = timestamps.clone();
        builder.pass("benchmark_start", vec![], move |ctx| unsafe {
            ctx.commands.reset_timestamps(&start)?;
            ctx.commands.timestamp(&start, 0)
        })?;
        let mut desc = TextureDesc::color(extent[0], extent[1], vk::Format::R8G8B8A8_UNORM);
        desc.usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        let output = builder.create_image(desc)?;
        renderer.render(&mut builder, camera, output)?;
        let end = timestamps.clone();
        builder.pass("benchmark_end", vec![], move |ctx| unsafe {
            ctx.commands.timestamp(&end, 1)
        })?;
        let (commands, timings) = builder.record_profiled(frame >= WARMUP)?;
        commands.submit()?.wait(10_000_000_000)?;
        if let Some(timings) = timings {
            let ticks = timestamps.read()?;
            let mut passes = timings.read()?;
            let ao_lighting = passes
                .iter()
                .filter(|(name, _)| {
                    matches!(
                        name.as_str(),
                        "ambient_occlusion" | "ao_upsample" | "lighting"
                    )
                })
                .map(|(_, ms)| ms)
                .sum();
            passes.push(("ao_and_lighting".into(), ao_lighting));
            passes.push((
                "gpu_frame".into(),
                timestamps.milliseconds(ticks[0], ticks[1]),
            ));
            for (pass, milliseconds) in passes {
                if pass.starts_with("benchmark_") {
                    continue;
                }
                writeln!(csv, "{},{pass},{milliseconds:.6}", frame - WARMUP)?;
                samples.entry(pass).or_default().push(milliseconds);
            }
        }
    }
    csv.flush()?;
    log::info!(
        "{}: {WARMUP} warm-up, {FRAMES} measured frames, {}x{}",
        path.display(),
        extent[0],
        extent[1]
    );
    for (pass, mut values) in samples {
        values.sort_by(f64::total_cmp);
        let median = (values[FRAMES / 2 - 1] + values[FRAMES / 2]) * 0.5;
        let p95 = values[(FRAMES * 95).div_ceil(100) - 1];
        log::info!("{pass}: median {median:.6} ms, p95 {p95:.6} ms");
    }
    Ok(())
}
