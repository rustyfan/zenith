use super::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires Vulkan validation, Slang, and supported GPU features"]
fn shader_reload_regenerates_brdf_tables_and_preserves_old_images() -> Result<()> {
    let instance = Instance::new(&[], true)?;
    anyhow::ensure!(
        instance.validation_enabled(),
        "IBL reload test requires validation"
    );
    let gpu = Gpu::new(instance.clone(), None)?;
    let descriptors = Descriptors::new(&gpu, 128, 16)?;
    let directory = std::env::temp_dir().join(format!("zenith-ibl-reload-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("lut.slang");
    let source = r#"struct Root { uint output; uint size; };
#include "zenith_root.slang"
[shader("compute")][numthreads(8,8,1)] void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= root.size)) return;
    RWTexture2D<float2> output = ResourceDescriptorHeap[NonUniformResourceIndex(root.output)];
    output[id.xy] = float2(0.25, 0.5);
}"#;
    std::fs::write(&path, source)?;
    let diffuse = gpu.compile_shader(
        "content/shaders/ibl_diffuse.slang",
        "main",
        ShaderStage::Compute,
    )?;
    let specular = gpu.compile_shader(
        "content/shaders/ibl_specular.slang",
        "main",
        ShaderStage::Compute,
    )?;
    let lut = gpu.compile_shader(&path, "main", ShaderStage::Compute)?;
    let average = gpu.compile_shader(
        "content/shaders/brdf_average.slang",
        "main",
        ShaderStage::Compute,
    )?;
    let sampler = descriptors.sampler(vk::Filter::LINEAR, vk::SamplerAddressMode::CLAMP_TO_EDGE)?;
    let mut ibl = ImageBasedLightingRenderer::new(
        &gpu,
        &descriptors,
        sampler,
        [&diffuse, &specular, &lut, &average],
    )?;
    let original = ibl.brdf_lut.view().texture().clone();
    let diffuse = ibl.buffer.clone();
    assert!(
        read_image(&gpu, &original, 0, 2)?
            .chunks_exact(2)
            .all(|pixel| pixel == [0.25, 0.5])
    );
    let generation = ibl.lut_pipeline.generation();
    std::fs::write(
        &path,
        source.replace("float2(0.25, 0.5)", "float2(0.5, 0.25)"),
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cache = ResourceCache::default();
    loop {
        let mut graph = RenderGraphBuilder::new(&gpu, &descriptors, &mut cache)?;
        ibl.render(&mut graph)?;
        drop(graph);
        if ibl.lut_pipeline.generation() > generation {
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "IBL shader reload timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
    let changed = ibl.brdf_lut.view().texture();
    assert!(!Arc::ptr_eq(&original, changed));
    assert!(Arc::ptr_eq(&diffuse, &ibl.buffer));
    assert!(
        read_image(&gpu, changed, 0, 2)?
            .chunks_exact(2)
            .all(|pixel| pixel == [0.5, 0.25])
    );
    assert!(
        read_image(&gpu, &original, 0, 2)?
            .chunks_exact(2)
            .all(|pixel| pixel == [0.25, 0.5])
    );
    let averages = read_image(&gpu, ibl.brdf_average.view().texture(), 0, 2)?;
    assert!(
        averages
            .chunks_exact(2)
            .all(|pixel| (pixel[0] - 0.5).abs() < 0.001 && (pixel[1] - 0.25).abs() < 0.001)
    );
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "IBL reload validation errors: {:?}",
        instance.validation_errors()
    );
    std::fs::remove_file(path)?;
    std::fs::remove_dir(directory)?;
    Ok(())
}
