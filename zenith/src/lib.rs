use crate::main_loop::EngineLoop;
use zenith_core::cli::EngineArgs;

mod app;
mod engine;
mod main_loop;

pub use app::{App, RenderContext, RenderableApp};
pub use engine::Engine;
pub use zenith_core::cli::EngineArgs as Args;

pub use paste::paste;

macro_rules! module_facade {
    ($name:ident) => {
        $crate::paste! {
            pub mod $name {
                pub use [<zenith_ $name>]::*;
            }
        }
    };
}

module_facade!(core);
module_facade!(asset);
module_facade!(rhi);
module_facade!(renderer);
module_facade!(rendergraph);

/// Launch main engine loop with specific App.
pub fn launch<A: RenderableApp>() -> Result<(), anyhow::Error> {
    let args = EngineArgs::parse_args();

    zenith_core::log::initialize(args.log_level.into())?;
    zenith_core::profile::initialize()?;
    zenith_asset::initialize()?;

    let app = A::new(&args)?;

    let main_loop = EngineLoop::new(app)?;
    main_loop.run()?;

    Ok(())
}
