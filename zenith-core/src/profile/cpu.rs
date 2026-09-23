use profiling::puffin;
use std::{
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
};

const IDLE: u8 = 0;
const REQUESTED: u8 = 1;
const RECORDING: u8 = 2;
static CAPTURE: AtomicU8 = AtomicU8::new(IDLE);

pub fn capture_next_frame() {
    let _ = CAPTURE.compare_exchange(IDLE, REQUESTED, Ordering::AcqRel, Ordering::Acquire);
}

pub fn capture_pending() -> bool {
    CAPTURE.load(Ordering::Acquire) != IDLE
}

pub fn is_recording() -> bool {
    CAPTURE.load(Ordering::Acquire) == RECORDING
}

#[must_use]
pub struct FrameCapture {
    _thread: PhantomData<*mut ()>,
}

pub fn begin_frame() -> Option<FrameCapture> {
    CAPTURE
        .compare_exchange(REQUESTED, RECORDING, Ordering::AcqRel, Ordering::Acquire)
        .ok()?;
    {
        let mut profiler = puffin::GlobalProfiler::lock();
        profiler.new_frame();
        profiler.emit_scope_snapshot();
    }
    puffin::set_scopes_on(true);
    Some(FrameCapture {
        _thread: PhantomData,
    })
}

impl Drop for FrameCapture {
    fn drop(&mut self) {
        puffin::set_scopes_on(false);
        puffin::GlobalProfiler::lock().new_frame();
        CAPTURE.store(IDLE, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
