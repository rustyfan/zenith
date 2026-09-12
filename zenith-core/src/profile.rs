pub fn initialize() -> anyhow::Result<()> {
    #[cfg(feature = "cpu-profiling")]
    profiling::puffin::set_scopes_on(true);
    Ok(())
}
