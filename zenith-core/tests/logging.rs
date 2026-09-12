use std::process::Command;
use zenith_core::log;

#[test]
fn logger_survives_initialization_and_resource_teardown() {
    for filter in ["info", "error"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "logger_child", "--nocapture"])
            .env("ZENITH_LOG_TEST_CHILD", "1")
            .env("RUST_LOG", filter)
            .env("RUST_LOG_STYLE", "never")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(
            stderr.contains("logger remains configured"),
            filter == "info"
        );
        assert_eq!(
            stderr.contains("resource destructor logged"),
            filter == "info"
        );
        assert!(stderr.contains("worker callback logged"), "{stderr}");
        assert!(stderr.contains("logging after teardown"), "{stderr}");
    }
}

#[test]
fn logger_child() {
    if std::env::var("ZENITH_LOG_TEST_CHILD").as_deref() != Ok("1") {
        return;
    }
    log::initialize(log::LevelFilter::Info).unwrap();
    assert!(log::initialize(log::LevelFilter::Off).is_err());
    log::info!("logger remains configured");

    struct Resource;
    impl Drop for Resource {
        fn drop(&mut self) {
            log::warn!("resource destructor logged");
        }
    }

    let resource = Resource;
    std::thread::spawn(|| log::error!("worker callback logged"))
        .join()
        .unwrap();
    drop(resource);
    log::error!("logging after teardown");
}
