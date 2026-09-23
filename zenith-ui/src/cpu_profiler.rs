use puffin_egui::GlobalProfilerUi;
use zenith_core::profile::cpu;

pub struct CpuProfiler {
    viewer: GlobalProfilerUi,
    displayed_frame: Option<u64>,
}

impl Default for CpuProfiler {
    fn default() -> Self {
        let viewer = GlobalProfilerUi::default();
        {
            let mut frames = viewer.global_frame_view().lock();
            frames.set_max_recent(1);
            frames.set_max_slow(0);
            frames.set_pack_frames(false);
        }
        Self {
            viewer,
            displayed_frame: None,
        }
    }
}

impl CpuProfiler {
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                !cpu::capture_pending(),
                egui::Button::new("Capture next frame"),
            )
            .clicked()
        {
            cpu::capture_next_frame();
        }
        ui.small(
            "Instrumented CPU scopes. Durations include waits; self time excludes child scopes.",
        );
        if cpu::is_recording() {
            ui.label("Capturing this frame...");
            return;
        }
        if cpu::capture_pending() {
            ui.label("Waiting for the next frame...");
        }
        let latest = self
            .viewer
            .global_frame_view()
            .lock()
            .latest_frame()
            .map(|frame| frame.frame_index());
        if latest != self.displayed_frame {
            self.viewer.profiler_ui.reset();
            self.displayed_frame = latest;
        }
        if latest.is_some() {
            ui.label("One frame captured. Recording is stopped while you inspect it.");
            self.viewer.ui(ui);
        } else {
            ui.label("Capture a frame to inspect its flame graph and scope statistics.");
        }
    }
}

#[cfg(test)]
mod tests;
