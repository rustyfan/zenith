use profiling::puffin;
use std::{collections::BTreeMap, io::Write};

pub struct Report {
    frames: puffin::GlobalFrameView,
    latest: Option<u64>,
    samples: u64,
    totals: BTreeMap<String, (u64, i64, i64)>,
}

impl Default for Report {
    fn default() -> Self {
        let frames = puffin::GlobalFrameView::default();
        frames.lock().set_max_recent(1);
        frames.lock().set_max_slow(0);
        frames.lock().set_pack_frames(false);
        Self {
            frames,
            latest: None,
            samples: 0,
            totals: BTreeMap::new(),
        }
    }
}

impl Report {
    pub fn capture(&mut self) {
        let view = self.frames.lock();
        if let Some(frame) = view
            .latest_frame()
            .filter(|f| Some(f.frame_index()) != self.latest)
        {
            self.latest = Some(frame.frame_index());
            self.samples += 1;
            for stream in frame.unpacked().unwrap().thread_streams.values() {
                Self::visit(&stream.stream, 0, view.scope_collection(), &mut self.totals);
            }
        }
        if self.samples < 64 {
            zenith::core::profile::cpu::capture_next_frame();
        }
    }

    fn visit(
        stream: &puffin::Stream,
        offset: u64,
        names: &puffin::ScopeCollection,
        totals: &mut BTreeMap<String, (u64, i64, i64)>,
    ) -> i64 {
        let mut elapsed = 0;
        for scope in puffin::Reader::with_offset(stream, offset).unwrap() {
            let scope = scope.unwrap();
            let children = if scope.child_begin_position < scope.child_end_position {
                Self::visit(stream, scope.child_begin_position, names, totals)
            } else {
                0
            };
            let name = names
                .fetch_by_id(&scope.id)
                .map(|s| s.name().to_string())
                .unwrap_or_default();
            let key = if scope.record.data.is_empty() {
                name
            } else {
                format!("{name}: {}", scope.record.data)
            };
            let total = totals.entry(key).or_default();
            total.0 += 1;
            total.1 += scope.record.duration_ns;
            total.2 += scope.record.duration_ns - children;
            elapsed += scope.record.duration_ns;
        }
        elapsed
    }

    pub fn write(&self, path: &str) {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        writeln!(
            file,
            "scope,calls_per_frame,inclusive_us_per_frame,self_us_per_frame"
        )
        .unwrap();
        for (name, &(calls, inclusive, exclusive)) in &self.totals {
            writeln!(
                file,
                "\"{}\",{:.3},{:.3},{:.3}",
                name.replace('"', "\"\""),
                calls as f64 / self.samples as f64,
                inclusive as f64 / self.samples as f64 / 1000.0,
                exclusive as f64 / self.samples as f64 / 1000.0
            )
            .unwrap();
        }
    }
}
