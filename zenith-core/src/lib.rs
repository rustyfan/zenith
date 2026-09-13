use std::path::PathBuf;
use std::sync::OnceLock;

pub mod log;
pub mod collections;
pub mod camera;
pub mod math;
pub mod input;
pub mod profile;
pub mod cli;
pub mod time;
pub mod color;

static WORKSPACE_ROOT_CACHE: OnceLock<PathBuf> = OnceLock::new();

pub fn workspace_root() -> &'static PathBuf {
    WORKSPACE_ROOT_CACHE.get_or_init(|| {
        // Get the directory where Cargo.toml for the workspace is located
        let mut current_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        loop {
            let cargo_toml = current_dir.join("Cargo.toml");
            if cargo_toml.exists() {
                if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                    if content.contains("[workspace]") {
                        return current_dir;
                    }
                }
            }
            if !current_dir.pop() {
                break;
            }
        }
        // Fallback to parent directory of current crate
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    })
}
