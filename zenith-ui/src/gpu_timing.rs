use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui};
use std::{
    collections::{hash_map::RandomState, BTreeMap, VecDeque},
    hash::{BuildHasher, BuildHasherDefault, DefaultHasher},
    time::{Duration, Instant},
};

const MAX_FRAMES: usize = 4096;

fn oklab(color: Color32) -> [f32; 3] {
    let [r, g, b, _] = egui::Rgba::from(color).to_array();
    // Linear sRGB -> Oklab, so distance reflects perceived color differences.
    let l = (0.41222146 * r + 0.53633255 * g + 0.051445995 * b).cbrt();
    let m = (0.2119035 * r + 0.6806995 * g + 0.10739696 * b).cbrt();
    let s = (0.08830246 * r + 0.28171885 * g + 0.6299787 * b).cbrt();
    [
        0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
        1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
        0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
    ]
}

fn color_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PassId {
    name: String,
    occurrence: usize,
}

impl PassId {
    fn label(&self) -> String {
        if self.occurrence == 1 {
            self.name.clone()
        } else {
            format!("{} #{}", self.name, self.occurrence)
        }
    }
}

struct Pass {
    color: Color32,
    visible: bool,
    last_frame: u64,
}

struct Frame {
    number: u64,
    captured_at: Instant,
    passes: BTreeMap<PassId, f64>,
}

pub struct GpuTimingGraph {
    frames: VecDeque<Frame>,
    passes: BTreeMap<PassId, Pass>,
    colors: BTreeMap<PassId, Color32>,
    color_seed: u64,
    window_seconds: f32,
    paused: bool,
}

impl Default for GpuTimingGraph {
    fn default() -> Self {
        Self {
            frames: VecDeque::new(),
            passes: BTreeMap::new(),
            colors: BTreeMap::new(),
            color_seed: RandomState::new().hash_one("gpu_timing"),
            window_seconds: 1.0,
            paused: false,
        }
    }
}

impl GpuTimingGraph {
    fn pass_color(&mut self, id: &PassId) -> Color32 {
        if let Some(&color) = self.colors.get(id) {
            return color;
        }
        let assigned = self
            .colors
            .values()
            .map(|&color| oklab(color))
            .collect::<Vec<_>>();
        let candidates = (0..256).map(|index| {
            let hash = BuildHasherDefault::<DefaultHasher>::default().hash_one((
                self.color_seed,
                id,
                index,
            ));
            Color32::from_rgb(hash as u8, (hash >> 8) as u8, (hash >> 16) as u8)
        });
        let mut best = Color32::from_rgb(255, 128, 64);
        let mut best_distance = -1.0;
        for color in candidates.chain(std::iter::once(best)) {
            let lab = oklab(color);
            if lab[0] < 0.65 || lab[1].hypot(lab[2]) < 0.08 {
                continue;
            }
            let distance = assigned
                .iter()
                .map(|&other| color_distance(lab, other))
                .fold(f32::INFINITY, f32::min);
            if distance > best_distance {
                best = color;
                best_distance = distance;
            }
        }
        self.colors.insert(id.clone(), best);
        best
    }

    pub fn push_frame(
        &mut self,
        frame_number: u64,
        captured_at: Instant,
        timings: &[(String, f64)],
    ) {
        if self.paused
            || self.frames.back().is_some_and(|frame| {
                frame_number <= frame.number || captured_at < frame.captured_at
            })
        {
            return;
        }
        let mut occurrences = BTreeMap::new();
        let mut passes = BTreeMap::new();
        for (name, ms) in timings {
            let occurrence = occurrences.entry(name).or_insert(0);
            *occurrence += 1;
            if !ms.is_finite() || *ms < 0.0 {
                continue;
            }
            let id = PassId {
                name: name.clone(),
                occurrence: *occurrence,
            };
            if let Some(pass) = self.passes.get_mut(&id) {
                pass.last_frame = frame_number;
            } else {
                let color = self.pass_color(&id);
                self.passes.insert(
                    id.clone(),
                    Pass {
                        color,
                        visible: true,
                        last_frame: frame_number,
                    },
                );
            }
            passes.insert(id, *ms);
        }
        self.frames.push_back(Frame {
            number: frame_number,
            captured_at,
            passes,
        });
        self.trim();
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.passes.clear();
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn window_seconds(&self) -> f32 {
        self.window_seconds
    }

    pub fn set_window_seconds(&mut self, seconds: f32) {
        if seconds.is_finite() {
            self.window_seconds = seconds.clamp(0.05, 60.0);
            self.trim();
        }
    }

    fn trim(&mut self) {
        let Some(latest) = self.frames.back() else {
            return;
        };
        let latest = latest.captured_at;
        let window = Duration::from_secs_f32(self.window_seconds);
        while self.frames.front().is_some_and(|frame| {
            latest.duration_since(frame.captured_at) > window || self.frames.len() > MAX_FRAMES
        }) {
            self.frames.pop_front();
        }
        if let Some(first) = self.frames.front() {
            self.passes
                .retain(|_, pass| pass.last_frame >= first.number);
        }
    }

    fn max_ms(&self) -> f64 {
        self.frames
            .iter()
            .flat_map(|frame| &frame.passes)
            .filter(|(id, _)| self.passes.get(*id).is_some_and(|pass| pass.visible))
            .map(|(_, &ms)| ms)
            .fold(0.01, f64::max)
            * 1.1
    }

    pub fn show(&mut self, ui: &mut Ui) -> Response {
        ui.push_id(ui.next_auto_id(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Window");
                let mut seconds = self.window_seconds;
                if ui
                    .add(
                        egui::DragValue::new(&mut seconds)
                            .range(0.05..=60.0)
                            .speed(0.05)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    self.set_window_seconds(seconds);
                }
                ui.checkbox(&mut self.paused, "Pause");
                if ui.button("Clear").clicked() {
                    self.clear();
                }
                if let (Some(first), Some(last)) = (self.frames.front(), self.frames.back()) {
                    ui.weak(format!(
                        "{} frames / {:.2} s",
                        self.frames.len(),
                        last.captured_at
                            .duration_since(first.captured_at)
                            .as_secs_f32()
                    ));
                }
            });
            self.total_bar(ui);
            let response = self.plot(ui);
            egui::ScrollArea::vertical()
                .id_salt("legend")
                .max_height(110.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (id, pass) in &mut self.passes {
                            let response = ui.selectable_label(pass.visible, id.label());
                            let rect = response.rect.shrink2(vec2(3.0, 0.0));
                            ui.painter().rect_filled(
                                Rect::from_min_max(
                                    pos2(rect.left(), rect.bottom() - 2.0),
                                    rect.max,
                                ),
                                0.0,
                                pass.color,
                            );
                            if response.clicked() {
                                pass.visible = !pass.visible;
                            }
                        }
                    });
                });
            response
        })
        .inner
    }

    fn total_bar(&self, ui: &mut Ui) {
        let total_ms = |frame: &Frame| frame.passes.values().sum::<f64>();
        let latest = self.frames.back().map(total_ms);
        let peak = self.frames.iter().map(total_ms).fold(0.0, f64::max);
        let fraction = (latest.unwrap_or(0.0) / peak.max(f64::EPSILON)) as f32;
        let text = match latest {
            Some(ms) => format!("Total GPU: {ms:.3} ms"),
            None => "Total GPU: waiting for samples".into(),
        };
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_height(22.0)
                .corner_radius(0)
                .text(text),
        )
        .on_hover_text(format!(
            "Sum of all measured passes, including hidden passes.\nBar scale: 0 to {peak:.3} ms (peak in this window)."
        ));
    }

    fn plot(&self, ui: &mut Ui) -> Response {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width().max(100.0), 240.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let plot = Rect::from_min_max(rect.min + vec2(52.0, 20.0), rect.max - vec2(8.0, 36.0));
        let text_color = ui.visuals().text_color();
        let font = FontId::monospace(11.0);
        painter.rect_filled(plot, 0.0, Color32::from_rgb(18, 22, 28));
        painter.text(
            rect.min,
            Align2::LEFT_TOP,
            "GPU ms",
            font.clone(),
            text_color,
        );
        painter.text(
            pos2(plot.center().x, rect.bottom()),
            Align2::CENTER_BOTTOM,
            "Frame",
            font.clone(),
            text_color,
        );
        let max_ms = self.max_ms();
        for tick in 0..=4 {
            let fraction = tick as f32 / 4.0;
            let y = plot.bottom() - fraction * plot.height();
            painter.line_segment(
                [pos2(plot.left(), y), pos2(plot.right(), y)],
                Stroke::new(1.0, Color32::from_gray(55)),
            );
            painter.text(
                pos2(plot.left() - 5.0, y),
                Align2::RIGHT_CENTER,
                format!("{:.2}", fraction as f64 * max_ms),
                font.clone(),
                text_color,
            );
        }
        let (Some(first), Some(last)) = (self.frames.front(), self.frames.back()) else {
            painter.text(
                plot.center(),
                Align2::CENTER_CENTER,
                "Waiting for GPU timings",
                font,
                Color32::LIGHT_GRAY,
            );
            return response;
        };
        let span = (last.number - first.number).max(1);
        let x = |number: u64| {
            plot.right() - ((last.number - number) as f64 / span as f64) as f32 * plot.width()
        };
        for (number, align) in [
            (first.number, Align2::LEFT_TOP),
            (first.number + span / 2, Align2::CENTER_TOP),
            (last.number, Align2::RIGHT_TOP),
        ] {
            if (number == last.number && align != Align2::RIGHT_TOP)
                || (number == first.number && align == Align2::CENTER_TOP)
            {
                continue;
            }
            painter.text(
                pos2(x(number), plot.bottom() + 4.0),
                align,
                number.to_string(),
                font.clone(),
                text_color,
            );
        }
        let lines = painter.with_clip_rect(plot);
        for (id, pass) in self.passes.iter().filter(|(_, pass)| pass.visible) {
            let mut previous = None;
            for frame in &self.frames {
                let Some(&ms) = frame.passes.get(id) else {
                    previous = None;
                    continue;
                };
                let point = pos2(
                    x(frame.number),
                    plot.bottom() - (ms / max_ms) as f32 * plot.height(),
                );
                if let Some((number, before)) = previous {
                    if frame.number == number + 1 {
                        lines.line_segment([before, point], Stroke::new(1.5, pass.color));
                    } else {
                        lines.circle_filled(point, 1.5, pass.color);
                    }
                } else {
                    lines.circle_filled(point, 1.5, pass.color);
                }
                previous = Some((frame.number, point));
            }
        }
        if let Some(pointer) = response.hover_pos().filter(|point| plot.contains(*point)) {
            let hovered = self
                .frames
                .iter()
                .min_by(|a, b| {
                    (x(a.number) - pointer.x)
                        .abs()
                        .total_cmp(&(x(b.number) - pointer.x).abs())
                })
                .unwrap();
            painter.line_segment(
                [
                    pos2(x(hovered.number), plot.top()),
                    pos2(x(hovered.number), plot.bottom()),
                ],
                Stroke::new(1.0, Color32::LIGHT_GRAY),
            );
            return response.on_hover_ui_at_pointer(|ui| {
                ui.label(format!("Frame {}", hovered.number));
                for (id, ms) in &hovered.passes {
                    let pass = &self.passes[id];
                    if pass.visible {
                        ui.horizontal(|ui| {
                            ui.colored_label(pass.color, "■");
                            ui.label(format!("{}: {:.3} ms", id.label(), ms));
                        });
                    }
                }
            });
        }
        response
    }
}

#[cfg(test)]
mod tests;
