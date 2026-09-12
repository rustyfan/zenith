use std::{path::Path, time::Instant};
use zenith_asset::{AssetServer, FileSource, mesh::Scene, texture::Texture};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: assets cook <source> <cache> <paths...> | inspect <cache> <paths...>")?;
    let mut builder = AssetServer::builder()
        .with_builtin_assets()
        .target(std::env::var("ZENITH_ASSET_TARGET").unwrap_or_else(|_| "desktop".into()));
    match mode.as_str() {
        "cook" => {
            if !cfg!(feature = "importers") {
                return Err("cooking built-in formats requires the importers feature".into());
            }
            builder = builder.source(FileSource::new(
                args.next().ok_or("missing source directory")?,
            ));
        }
        "inspect" => builder = builder.packaged(),
        _ => return Err("expected cook or inspect".into()),
    }
    let server = builder
        .cache_dir(args.next().ok_or("missing cache directory")?)
        .build()?;
    let paths: Vec<_> = args.collect();
    if paths.is_empty() {
        return Err("provide at least one source-relative asset path".into());
    }
    for path in paths {
        let started = Instant::now();
        let extension = Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "gltf" | "glb") {
            let scene = server.load_blocking::<Scene>(&path)?;
            println!(
                "{path}: {} nodes, {} instances ({:?})",
                scene.get().unwrap().nodes.len(),
                scene.get().unwrap().instances.len(),
                started.elapsed()
            );
        } else {
            let texture = server.load_blocking::<Texture>(&path)?;
            let data = texture.get().unwrap();
            println!(
                "{path}: {}x{}, {:?}, {} mips, {} bytes ({:?})",
                data.width,
                data.height,
                data.format,
                data.mip_levels,
                data.pixels.len(),
                started.elapsed()
            );
        }
    }
    println!("{:?}", server.stats());
    Ok(())
}
