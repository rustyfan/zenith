use super::*;

#[test]
fn captures_complete_nested_frames_only_on_request_and_stops_on_unwind() {
    crate::profile::initialize().unwrap();
    let frames = puffin::GlobalFrameView::default();
    frames.lock().set_max_recent(1);
    frames.lock().set_max_slow(0);
    assert!(begin_frame().is_none());
    assert!(!puffin::are_scopes_on());

    let mut previous = None;
    for _ in 0..2 {
        capture_next_frame();
        assert!(capture_pending());
        {
            let _frame = begin_frame().unwrap();
            assert!(is_recording());
            assert!(begin_frame().is_none());
            capture_next_frame();
            profiling::scope!("test frame");
            for _ in 0..3 {
                profiling::scope!("test child");
                std::hint::black_box(42);
            }
            assert_eq!(
                frames.lock().latest_frame().map(|f| f.frame_index()),
                previous
            );
        }
        assert!(!capture_pending());
        assert!(!puffin::are_scopes_on());
        assert!(begin_frame().is_none());
        let view = frames.lock();
        assert_eq!(view.all_uniq().count(), 1);
        let frame = view.latest_frame().unwrap();
        assert_ne!(Some(frame.frame_index()), previous);
        previous = Some(frame.frame_index());
        let data = frame.unpacked().unwrap();
        let root_id = view.scope_collection().fetch_by_name("test frame").unwrap();
        let stream = data
            .thread_streams
            .values()
            .find(|stream| {
                puffin::Reader::from_start(&stream.stream)
                    .any(|scope| scope.unwrap().id == *root_id)
            })
            .unwrap();
        let roots = puffin::Reader::from_start(&stream.stream)
            .read_top_scopes()
            .unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(stream.num_scopes, 4);
        assert_eq!(stream.depth, 2);
        assert_eq!(roots[0].record.duration_ns, frame.duration_ns());
        let children = puffin::Reader::with_offset(&stream.stream, roots[0].child_begin_position)
            .unwrap()
            .read_top_scopes()
            .unwrap();
        assert_eq!(children.len(), 3);
        assert!(children
            .iter()
            .all(|child| child.record.stop_ns() <= roots[0].record.stop_ns()));
    }

    capture_next_frame();
    assert!(std::panic::catch_unwind(|| {
        let _frame = begin_frame().unwrap();
        profiling::scope!("unwinding frame");
        panic!("test capture cleanup");
    })
    .is_err());
    assert!(!capture_pending());
    assert!(!puffin::are_scopes_on());
}
