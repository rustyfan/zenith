use zenith_core::log;
fn main() -> anyhow::Result<()> {
    log::initialize(log::LevelFilter::Info)?;
    let instance = zenith_rhi::Instance::new(&[], true)?;
    log::info!("Validation: {}", instance.validation_enabled());
    for adapter in instance.adapters()? {
        log::info!("{adapter:#?}");
    }
    anyhow::ensure!(
        instance.validation_errors().is_empty(),
        "Vulkan validation errors"
    );
    Ok(())
}
