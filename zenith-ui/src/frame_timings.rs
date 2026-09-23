use egui::{Color32, ProgressBar, Response, Ui};

struct Sample {
    number: u64,
    cpu_ms: f64,
    gpu_ms: Option<f64>,
}

#[derive(Default)]
pub struct FrameTimings {
    latest: Option<Sample>,
}

impl FrameTimings {
    pub fn set_frame(&mut self, frame_number: u64, cpu_ms: f64, gpu_ms: Option<f64>) {
        if !cpu_ms.is_finite()
            || cpu_ms < 0.0
            || self
                .latest
                .as_ref()
                .is_some_and(|sample| frame_number <= sample.number)
        {
            return;
        }
        self.latest = Some(Sample {
            number: frame_number,
            cpu_ms,
            gpu_ms: gpu_ms.filter(|ms| ms.is_finite() && *ms >= 0.0),
        });
    }

    pub fn show(&self, ui: &mut Ui) -> Response {
        ui.vertical(|ui| {
            let Some(sample) = &self.latest else {
                ui.label("Waiting for a completed frame...");
                return;
            };
            ui.label(format!("Completed frame {}", sample.number));
            let scale = sample.cpu_ms.max(sample.gpu_ms.unwrap_or(0.0)).max(0.001);
            ui.add(
                ProgressBar::new((sample.cpu_ms / scale) as f32)
                    .desired_height(28.0)
                    .fill(Color32::from_rgb(75, 180, 245))
                    .text(format!("Total CPU: {:.3} ms", sample.cpu_ms)),
            )
            .on_hover_text("Main-thread update and render graph build/record time. Excludes GPU waits, swapchain acquire/present, and background workers.");
            ui.add(
                ProgressBar::new((sample.gpu_ms.unwrap_or(0.0) / scale) as f32)
                    .desired_height(28.0)
                    .fill(Color32::from_rgb(245, 175, 65))
                    .text(match sample.gpu_ms {
                        Some(ms) => format!("Total GPU: {ms:.3} ms"),
                        None => "Total GPU: waiting for a profiled frame...".to_owned(),
                    }),
            )
            .on_hover_text("Sum of all measured render passes for the same frame, including hidden passes.");
            ui.weak(format!("Shared scale: 0 to {scale:.3} ms"));
            ui.small("CPU: main-thread update + render recording.");
            ui.small("GPU: all measured render passes.");
        })
        .response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_keep_frames_paired_and_handle_missing_or_invalid_samples() {
        let mut timings = FrameTimings::default();
        timings.set_frame(3, 2.0, Some(4.0));
        for (number, cpu_ms) in [
            (2, 9.0),
            (3, 9.0),
            (4, f64::NAN),
            (4, -1.0),
            (4, f64::INFINITY),
        ] {
            timings.set_frame(number, cpu_ms, Some(9.0));
        }
        let sample = timings.latest.as_ref().unwrap();
        assert_eq!(
            (sample.number, sample.cpu_ms, sample.gpu_ms),
            (3, 2.0, Some(4.0))
        );
        timings.set_frame(4, 1.0, None);
        let sample = timings.latest.as_ref().unwrap();
        assert_eq!(
            (sample.number, sample.cpu_ms, sample.gpu_ms),
            (4, 1.0, None)
        );
        timings.set_frame(5, 0.0, Some(f64::NAN));
        assert_eq!(timings.latest.as_ref().unwrap().gpu_ms, None);
        timings.set_frame(6, 0.0, Some(0.0));
        let context = egui::Context::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 240.0),
                )),
                ..Default::default()
            },
            |ui| {
                timings.show(ui);
            },
        );
        output.textures_delta.clear();
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        assert!(!primitives.is_empty());
        for primitive in primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                assert!(mesh.vertices.iter().all(|vertex| vertex.pos.is_finite()));
            }
        }
    }
}
