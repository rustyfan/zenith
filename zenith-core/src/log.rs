pub use log::{debug, error, info, trace, warn, LevelFilter};

pub fn initialize(level: LevelFilter) -> Result<(), anyhow::Error> {
    env_logger::builder()
        .filter_level(level)
        .parse_default_env()
        .try_init()?;

    Ok(())
}
