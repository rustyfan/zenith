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
        "import values; struct Root { uint* output; }; [[vk::binding(0, 0)]] ConstantBuffer<Root> root; [shader(\"compute\")] [numthreads(1,1,1)] void main() { root.output[0] = changed_value(); }",
    )?;
    let output = gpu.allocate(4, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(gpu, 256)?;
    let root = arguments.push(&output.address())?;
    let mut previous = None;
    for value in [42u32, 99] {
        std::fs::write(
            directory.join("values.slang"),
            format!("uint changed_value() {{ return {value}; }}"),
        )?;
        let shader = gpu.compile_shader(&path, "main", ShaderStage::Compute)?;
        let pipeline = gpu.compute(&shader)?;
        ensure!(
            previous.as_ref().is_none_or(|p| !Arc::ptr_eq(p, &pipeline)),
            "changed include reused old pipeline"
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
            u32::from_ne_bytes(actual) == value,
            "shader include update was not reflected on GPU"
        );
        previous = Some(pipeline);
    }
    std::fs::write(directory.join("values.slang"), "invalid slang syntax")?;
    let diagnostic = gpu
        .compile_shader(&path, "main", ShaderStage::Compute)
        .err()
        .expect("shader error was not reported")
        .to_string();
    ensure!(
        diagnostic.contains("values.slang"),
        "missing imported source diagnostic: {diagnostic}"
    );
    log::info!(
        "PASS: shader include invalidation, pipeline content identity and compiler diagnostics"
    );
    Ok(())
}
