use crate::main_loop::EngineLoop;
use zenith_core::cli::EngineArgs;

mod app;
mod engine;
mod main_loop;

pub use app::{App, RenderContext, RenderableApp};
pub use engine::Engine;
pub use zenith_core::cli::EngineArgs as Args;

pub use zenith_asset as asset;
pub use zenith_core as core;
pub use zenith_renderer as renderer;
pub use zenith_rendergraph as rendergraph;
pub use zenith_rhi as rhi;

/// Launch main engine loop with specific App.
pub fn launch<A: RenderableApp>() -> Result<(), anyhow::Error> {
    zenith_core::profile::begin_startup();
    let args = EngineArgs::parse_args();

    zenith_core::log::initialize(args.log_level.into())?;
    zenith_core::profile::initialize()?;

    let app = A::new(&args)?;

    let main_loop = EngineLoop::new(app)?;
    main_loop.run()?;

    Ok(())
}
