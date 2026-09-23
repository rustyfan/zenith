pub use profiling::scope;

#[cfg(feature = "cpu-profiling")]
pub mod cpu;

pub fn initialize() -> anyhow::Result<()> {
    #[cfg(feature = "cpu-profiling")]
    profiling::puffin::set_scopes_on(false);
    Ok(())
}

use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Startup {
    start: Instant,
    epoch_ms: f64,
    path: PathBuf,
    presented: Mutex<Option<Duration>>,
    completed: std::sync::atomic::AtomicBool,
}
static STARTUP: OnceLock<Option<Startup>> = OnceLock::new();

pub fn begin_startup() {
    STARTUP.get_or_init(|| {
        std::env::var_os("ZENITH_STARTUP_PROFILE").map(|path| Startup {
            start: Instant::now(),
            epoch_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                * 1000.0,
            path: path.into(),
            presented: Mutex::new(None),
            completed: std::sync::atomic::AtomicBool::new(false),
        })
    });
}
pub fn startup_presented() {
    if let Some(Some(startup)) = STARTUP.get() {
        if startup.completed.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        startup
            .presented
            .lock()
            .unwrap()
            .get_or_insert_with(|| startup.start.elapsed());
    }
}
pub fn startup_needs_completion() -> bool {
    STARTUP
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|startup| {
            !startup.completed.load(std::sync::atomic::Ordering::Relaxed)
                && startup.presented.lock().unwrap().is_some()
        })
}
pub fn complete_startup() {
    let Some(Some(startup)) = STARTUP.get() else {
        return;
    };
    let completed = startup.start.elapsed().as_secs_f64() * 1000.0;
    let Some(presented) = *startup.presented.lock().unwrap() else {
        return;
    };
    let presented = presented.as_secs_f64() * 1000.0;
    startup
        .completed
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let csv = format!("name,start_ms,duration_ms\nentry_epoch_ms,{:.6},0\npresent_accepted,{presented:.6},0\nfirst_present_return,{presented:.6},0\nfirst_gpu_complete,{completed:.6},0\n", startup.epoch_ms);
    if let Err(error) = std::fs::write(&startup.path, csv) {
        crate::log::warn!("Startup profile write failed: {error}");
    }
}
