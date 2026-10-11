use anyhow::{Result, ensure};
use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

pub fn run(gpu: &Arc<Gpu>) -> Result<()> {
    let directory = std::path::Path::new("target/shader reload");
    std::fs::create_dir_all(directory)?;
    let path = directory.join("reload.slang");
    std::fs::write(
        &path,
        "import values; struct Root { uint* output; }; \n#include \"zenith_root.slang\"\n [shader(\"compute\")] [numthreads(1,1,1)] void main() { root.output[0] = changed_value(); }",
    )?;
    let output = gpu.allocate(4, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(gpu, 256)?;
    let root = arguments.push(&output.address())?;
    let values = directory.join("values.slang");
    std::fs::write(&values, "uint changed_value() { return 42; }")?;
    let shader = gpu.compile_shader(&path, "main", ShaderStage::Compute)?;
    let pipeline = gpu.compute(&shader)?;
    for value in [42u32, 99, 123] {
        let generation = pipeline.generation();
        std::fs::write(
            &values,
            format!("uint changed_value() {{ return {value}; }}"),
        )?;
        if value != 42 {
            wait_until(|| {
                drop(gpu.shader_reload_scope());
                pipeline.generation() > generation
            })?;
        }
        let mut commands = gpu.commands()?;
        unsafe {
            commands.dispatch(&pipeline, &root, [1, 1, 1], &[output.whole()])?;
        }
        commands.barrier(Access::COMPUTE_WRITE, Access::HOST_READ)?;
        commands.submit()?.wait(10_000_000_000)?;
        let mut actual = [0u8; 4];
        output.read(0, &mut actual)?;
        ensure!(
            u32::from_ne_bytes(actual) == value,
            "shader include update was not reflected on GPU"
        );
    }
    let generation = pipeline.generation();
    std::fs::write(&values, "invalid slang syntax")?;
    wait_until(|| gpu.shader_reload_error().is_some())?;
    let diagnostic = gpu.shader_reload_error().unwrap().message;
    ensure!(
        diagnostic.contains("values.slang"),
        "missing imported source diagnostic: {diagnostic}"
    );
    ensure!(
        pipeline.generation() == generation,
        "failed reload replaced the pipeline"
    );
    let mut commands = gpu.commands()?;
    unsafe {
        commands.dispatch(&pipeline, &root, [1, 1, 1], &[output.whole()])?;
    }
    commands.barrier(Access::COMPUTE_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut actual = [0u8; 4];
    output.read(0, &mut actual)?;
    ensure!(
        u32::from_ne_bytes(actual) == 123,
        "failed reload did not retain previous GPU output"
    );
    std::fs::write(&values, "uint changed_value() { return 456; }")?;
    wait_until(|| {
        drop(gpu.shader_reload_scope());
        pipeline.generation() > generation
    })?;
    log::info!(
        "PASS: automatic shader reload, compiler diagnostics, fallback and recovery with an existing pipeline"
    );
    Ok(())
}

fn wait_until(mut predicate: impl FnMut() -> bool) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !predicate() {
        ensure!(
            std::time::Instant::now() < deadline,
            "shader reload timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(())
}
