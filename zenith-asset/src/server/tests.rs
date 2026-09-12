#![cfg(feature = "parallel")]

use super::*;
use std::sync::{Condvar, Mutex as StdMutex, atomic::AtomicUsize};
use std::time::Instant;

fn manifest() -> Manifest {
    Manifest {
        format: 2,
        source: AssetAddress::parse("parallel.bundle").unwrap(),
        importer: "test.bundle".into(),
        importer_version: 1,
        settings: serde_json::Value::Null,
        target: "desktop".into(),
        inputs: Vec::new(),
        outputs: (0..2)
            .map(|i| {
                (
                    i.to_string(),
                    Output {
                        type_key: "test.number".into(),
                        schema: 1,
                        codec: "test".into(),
                        blob: String::new(),
                        length: i,
                        dependencies: Vec::new(),
                    },
                )
            })
            .collect(),
    }
}

#[test]
fn parallel_payloads_overlap_and_join_on_success_error_and_panic() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let manifest = manifest();
    for failure in 0..3 {
        let entered = StdMutex::new(0);
        let changed = Condvar::new();
        let completed = AtomicUsize::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.install(|| {
                read_payloads(&manifest, |output| {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut count = entered.lock().unwrap();
                    *count += 1;
                    changed.notify_all();
                    while *count < 2 {
                        let (next, timeout) = changed
                            .wait_timeout(count, deadline.saturating_duration_since(Instant::now()))
                            .unwrap();
                        count = next;
                        assert!(
                            !timeout.timed_out() || *count == 2,
                            "independent payloads did not overlap"
                        );
                    }
                    drop(count);
                    completed.fetch_add(1, Ordering::Relaxed);
                    match (output.length, failure) {
                        (0, 1) => Err(AssetError::new(ErrorKind::Cache, "test read failure")),
                        (0, 2) => panic!("test read panic"),
                        _ => Ok(vec![output.length as u8]),
                    }
                })
            })
        }));
        assert_eq!(completed.load(Ordering::Relaxed), 2);
        match failure {
            0 => {
                let bytes = result.unwrap().unwrap();
                assert_eq!(&**bytes.get("0").unwrap(), &[0]);
                assert_eq!(&**bytes.get("1").unwrap(), &[1]);
            }
            1 => assert!(
                result
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("parallel.bundle output \"0\"")
            ),
            _ => assert!(result.is_err()),
        }
        assert_eq!(
            pool.install(|| read_payloads(&manifest, |_| Ok(vec![42])))
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn one_worker_can_prepare_multiple_payloads() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let bytes = pool
        .install(|| read_payloads(&manifest(), |output| Ok(vec![output.length as u8])))
        .unwrap();
    assert_eq!(
        bytes.keys().map(String::as_str).collect::<Vec<_>>(),
        ["0", "1"]
    );
}
