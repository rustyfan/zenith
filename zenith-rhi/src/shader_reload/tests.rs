use super::*;
use crate::{
    Access, Arguments, Attachment, GraphicsMode, Instance, MemoryDomain, MemorySlice,
    ShaderBindingTableDesc, ShaderOptions, TextureDesc,
};
use anyhow::ensure;
use std::sync::atomic::{AtomicU64, Ordering};

static IDS: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    directory: PathBuf,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "zenith-reload-{}-{}",
            std::process::id(),
            IDS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("main.slang");
        std::fs::write(&path, "original")?;
        Ok(Self {
            directory: directory.canonicalize()?,
            path: path.canonicalize()?,
        })
    }

    fn shader(&self) -> Shader {
        let mut command = Command::new("unused-test-compiler");
        command.arg(&self.path);
        let entry = CString::new("main").unwrap();
        Shader {
            code: vec![0x07230203, 0, 0, 0, 0],
            stage: ShaderStage::Compute,
            entry: entry.clone(),
            heap_strides: (0, 0),
            source: None,
            initial_watch: None,
            request: Some(Arc::new(CompileRequest::new(
                &command,
                &self.path,
                entry,
                ShaderStage::Compute,
                (0, 0),
                None,
            ))),
            dependencies: vec![self.path.clone()],
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.parent() == std::env::temp_dir().canonicalize().ok().as_deref()
            && self
                .directory
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("zenith-reload-"))
        {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}

#[test]
fn content_changes_and_failed_imports_are_detected_without_timestamp_changes() -> Result<()> {
    let fixture = Fixture::new()?;
    let shader = fixture.shader();
    let request = shader.request.as_ref().unwrap();
    let before = Watch::read(request, &shader.dependencies);
    let modified = std::fs::metadata(&fixture.path)?.modified()?;
    std::fs::write(&fixture.path, "modified")?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.path)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    let after = Watch::read(request, &shader.dependencies);
    assert!(after.changed(&before, false));
    let unknown = fixture.directory.join("unknown.slang");
    std::fs::write(&unknown, "first")?;
    let before = Watch::read(request, &shader.dependencies);
    assert!(before.changed(&after, false));
    std::fs::write(&unknown, "second")?;
    let after = Watch::read(request, &shader.dependencies);
    assert!(!after.changed(&before, false));
    assert!(after.changed(&before, true));
    std::fs::remove_file(&unknown)?;
    assert!(Watch::read(request, &shader.dependencies).changed(&after, false));
    std::fs::remove_file(&fixture.path)?;
    let deleted = Watch::read(request, &shader.dependencies);
    assert!(deleted.changed(&after, false));
    std::fs::write(&fixture.path, "restored")?;
    assert!(Watch::read(request, &shader.dependencies).changed(&deleted, false));
    Ok(())
}

fn staged(reload: &Arc<Reload>, source: &Arc<Source>) -> Batch {
    let mut shader = source.current.read().clone();
    shader.code[1] += 1;
    Batch {
        sources: vec![(Arc::downgrade(source), shader)],
        updates: Vec::new(),
        watches: vec![Watched {
            source: Arc::downgrade(source),
            dependencies: source.current.read().dependencies.clone(),
            watch: source.initial.clone(),
        }],
        registration: reload.state.lock().registration,
    }
}

#[test]
fn publication_waits_for_recording_and_discards_stale_or_incomplete_batches() -> Result<()> {
    let fixture = Fixture::new()?;
    let reload = Arc::new(Reload::default());
    let shader = reload.track_shader(fixture.shader())?;
    let source = shader.source.as_ref().unwrap();
    let scope = reload.begin();
    let batch = staged(&reload, source);
    reload.state.lock().ready = Some(batch);
    drop(reload.begin());
    assert_eq!(source.current.read().code[1], 0);
    drop(scope);
    drop(reload.begin());
    assert_eq!(source.current.read().code[1], 1);
    assert_eq!(reload.state.lock().generation, 1);
    let batch = staged(&reload, source);
    reload.state.lock().ready = Some(batch);
    std::fs::write(&fixture.path, "new save")?;
    drop(reload.begin());
    assert_eq!(source.current.read().code[1], 1);
    assert!(reload.state.lock().retry.contains(&source.id()));
    let mut batch = staged(&reload, source);
    batch.watches[0].watch = Watch::read(&source.request, &source.current.read().dependencies);
    reload.state.lock().ready = Some(batch);
    reload.state.lock().registration += 1;
    drop(reload.begin());
    assert_eq!(reload.state.lock().generation, 1);
    let weak = Arc::downgrade(&reload);
    drop(shader);
    drop(reload);
    assert!(weak.upgrade().is_none());
    Ok(())
}

#[test]
fn explicit_compilation_does_not_reuse_a_source_with_different_active_code() -> Result<()> {
    let fixture = Fixture::new()?;
    let reload = Arc::new(Reload::default());
    let first = reload.track_shader(fixture.shader())?;
    first.source.as_ref().unwrap().current.write().code[1] = 99;
    let second = reload.track_shader(fixture.shader())?;
    assert!(!Arc::ptr_eq(
        first.source.as_ref().unwrap(),
        second.source.as_ref().unwrap()
    ));
    assert_eq!(second.snapshot().code[1], 0);
    Ok(())
}

#[test]
fn registration_preserves_changes_made_during_initial_compilation() -> Result<()> {
    let fixture = Fixture::new()?;
    let reload = Arc::new(Reload::default());
    let mut shader = fixture.shader();
    let request = shader.request.as_ref().unwrap();
    let started = std::time::SystemTime::now();
    let before = Watch::read(request, &shader.dependencies);
    std::fs::write(&fixture.path, "saved during compilation")?;
    shader.initial_watch = Some(before.compiled(&shader.dependencies, started));
    let shader = reload.track_shader(shader)?;
    let source = shader.source.as_ref().unwrap();
    assert!(Watch::read(&source.request, &shader.dependencies).changed(&source.initial, false));
    Ok(())
}

#[test]
fn recordings_started_before_the_first_shader_also_defer_publication() -> Result<()> {
    let fixture = Fixture::new()?;
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let recording = ShaderReloadScope {
        _reload: None,
        active: active.clone(),
    };
    let reload = Arc::new(Reload {
        active,
        ..Reload::default()
    });
    let shader = reload.track_shader(fixture.shader())?;
    let source = shader.source.as_ref().unwrap();
    let batch = staged(&reload, source);
    reload.state.lock().ready = Some(batch);
    drop(reload.begin());
    assert_eq!(source.current.read().code[1], 0);
    drop(recording);
    drop(reload.begin());
    assert_eq!(source.current.read().code[1], 1);
    Ok(())
}

fn wait_until(mut predicate: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !predicate() {
        ensure!(Instant::now() < deadline, "shader reload timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn compute_output(gpu: &Arc<Gpu>, pipeline: &Arc<ComputePipeline>) -> Result<u32> {
    let output = gpu.allocate(4, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(gpu, 64)?;
    let root = arguments.push(&output.address())?;
    let mut commands = gpu.commands()?;
    unsafe {
        commands.dispatch(pipeline, &root, [1, 1, 1], &[output.whole()])?;
    }
    commands.barrier(Access::COMPUTE_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut bytes = [0; 4];
    output.read(0, &mut bytes)?;
    Ok(u32::from_ne_bytes(bytes))
}

const SOURCE: &str = r#"import values;
struct Root { uint* output; };
#include "zenith_root.slang"
[shader("compute")][numthreads(1,1,1)] void compute_main() {
#ifdef DOUBLE_VARIANT
    root.output[0] = changed_value() * 2;
#else
    root.output[0] = changed_value();
#endif
}
[shader("vertex")] float4 vertex_main(uint id : SV_VertexID) : SV_Position { float2 p = float2((id << 1) & 2, id & 2); return float4(p * 2 - 1, 0, 1); }
[shader("fragment")] float4 fragment_main() : SV_Target { return float4(changed_value() / 255.0, 0, 0, 1); }
[shader("raygeneration")] void raygen_main() { root.output[0] = changed_value(); }
"#;

#[test]
#[ignore = "requires Slang, a Vulkan ray tracing device and validation"]
fn saved_shaders_reload_transactionally_and_recover_with_existing_handles() -> Result<()> {
    let _ = log::initialize(log::LevelFilter::Info);
    let fixture = Fixture::new()?;
    std::fs::write(&fixture.path, SOURCE)?;
    let values = fixture.directory.join("values.slang");
    std::fs::write(&values, "public uint changed_value() { return 7; }")?;
    let instance = Instance::new(&[], true)?;
    ensure!(
        instance.validation_enabled(),
        "reload test requires validation"
    );
    let gpu = Gpu::create(instance.clone(), None, None, GraphicsMode::Full)?;
    let empty_recording = gpu.commands()?;
    assert!(gpu.shader_reload.lock().upgrade().is_none());
    let compute = gpu.compile_shader(&fixture.path, "compute_main", ShaderStage::Compute)?;
    let double = gpu.compile_shader_with_options(
        &fixture.path,
        "compute_main",
        ShaderStage::Compute,
        ShaderOptions {
            defines: &[("DOUBLE_VARIANT", "1")],
            capabilities: &[],
        },
    )?;
    let vertex = gpu.compile_shader(&fixture.path, "vertex_main", ShaderStage::Vertex)?;
    let fragment = gpu.compile_shader(&fixture.path, "fragment_main", ShaderStage::Fragment)?;
    let ray = gpu.compile_shader(&fixture.path, "raygen_main", ShaderStage::RayGeneration)?;
    let pipeline = gpu.compute(&compute)?;
    let doubled = gpu.compute_specialized(&double, &[(3, 1)])?;
    let raster = gpu.raster(&RasterDesc {
        vertex: &vertex,
        fragment: &fragment,
        colors: &[vk::Format::R8G8B8A8_UNORM],
        depth: vk::Format::UNDEFINED,
        stencil: vk::Format::UNDEFINED,
        samples: vk::SampleCountFlags::TYPE_1,
        topology: vk::PrimitiveTopology::TRIANGLE_LIST,
        blend: &[Blend::default()],
        dynamic_blend: false,
    })?;
    let ray_pipeline = gpu.ray_tracing(&RayTracingDesc {
        shaders: &[&ray],
        groups: &[RayTracingShaderGroup::General { shader: 0 }],
        max_recursion_depth: 1,
    })?;
    let table = gpu.shader_binding_table(
        &ray_pipeline,
        &ShaderBindingTableDesc {
            raygen: 0,
            miss: &[],
            hit: &[],
            callable: &[],
        },
    )?;
    let mut request = (**compute.request.as_ref().unwrap()).clone();
    request.cache = None;
    let uncached = gpu.reload_service().track_shader(request.compile()?)?;
    assert!(
        uncached
            .dependencies
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == "values.slang"))
    );
    let uncached_pipeline = gpu.compute(&uncached)?;
    drop(empty_recording);
    assert_eq!(compute_output(&gpu, &pipeline)?, 7);
    let old_version = Arc::downgrade(&pipeline.current.read());
    let output = gpu.allocate(4, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(&gpu, 64)?;
    let root = arguments.push(&output.address())?;
    let mut old_commands = gpu.commands()?;
    unsafe {
        old_commands.dispatch(&pipeline, &root, [1, 1, 1], &[output.whole()])?;
    }
    old_commands.barrier(Access::COMPUTE_WRITE, Access::HOST_READ)?;
    std::fs::write(&values, "public uint changed_value() { return 9; }")?;
    wait_until(|| gpu.reload_service().state.lock().ready.is_some())?;
    drop(gpu.commands()?);
    assert_eq!(pipeline.generation(), 0);
    let mut submission = old_commands.submit()?;
    wait_until(|| {
        drop(gpu.shader_reload_scope());
        pipeline.generation() > 0
    })?;
    assert!(old_version.upgrade().is_some());
    submission.wait(10_000_000_000)?;
    let mut bytes = [0; 4];
    output.read(0, &mut bytes)?;
    assert_eq!(u32::from_ne_bytes(bytes), 7);
    assert!(old_version.upgrade().is_none());
    assert_eq!(compute_output(&gpu, &pipeline)?, 9);
    assert_eq!(compute_output(&gpu, &doubled)?, 18);
    assert_eq!(compute_output(&gpu, &uncached_pipeline)?, 9);
    assert_eq!(pipeline.generation(), raster.generation());
    assert_eq!(pipeline.generation(), ray_pipeline.generation());
    assert_eq!(pipeline.generation(), table.generation());
    assert!(Arc::ptr_eq(
        &table.current.read().pipeline,
        &ray_pipeline.current.read()
    ));
    assert_eq!(ray_output(&gpu, &table)?, 9);
    assert_eq!(raster_output(&gpu, &raster)?, 9);
    let generation = pipeline.generation();
    std::fs::write(
        &values,
        "public uint changed_value() {\n#ifdef DOUBLE_VARIANT\nreturn missing_identifier;\n#else\nreturn 31;\n#endif\n}",
    )?;
    wait_until(|| gpu.shader_reload_error().is_some())?;
    let error = gpu.shader_reload_error().unwrap();
    assert!(error.message.contains("values.slang") && error.message.contains("missing_identifier"));
    std::thread::sleep(POLL * 3);
    assert_eq!(gpu.shader_reload_error().unwrap().revision, error.revision);
    assert_eq!(pipeline.generation(), generation);
    assert_eq!(compute_output(&gpu, &pipeline)?, 9);
    assert_eq!(ray_output(&gpu, &table)?, 9);
    std::fs::write(&values, "public uint changed_value() { return 23; }")?;
    wait_until(|| {
        drop(gpu.shader_reload_scope());
        pipeline.generation() > generation
    })?;
    assert_eq!(compute_output(&gpu, &pipeline)?, 23);
    assert!(gpu.shader_reload_error().is_none());
    let generation = pipeline.generation();
    std::fs::write(
        &fixture.path,
        SOURCE.replace("import values;", "import replacement;"),
    )?;
    wait_until(|| gpu.shader_reload_error().is_some())?;
    assert_eq!(compute_output(&gpu, &pipeline)?, 23);
    let replacement = fixture.directory.join("replacement.slang");
    std::fs::write(&replacement, "public uint changed_value() { return 77; }")?;
    wait_until(|| {
        drop(gpu.shader_reload_scope());
        pipeline.generation() > generation
    })?;
    assert_eq!(compute_output(&gpu, &pipeline)?, 77);
    let generation = pipeline.generation();
    for value in [88, 99, 130] {
        let temporary = fixture.directory.join("save.tmp");
        std::fs::write(
            &temporary,
            format!("public uint changed_value() {{ return {value}; }}"),
        )?;
        std::fs::rename(temporary, &replacement)?;
        std::thread::sleep(Duration::from_millis(10));
    }
    wait_until(|| {
        drop(gpu.shader_reload_scope());
        pipeline.generation() > generation
    })?;
    assert_eq!(compute_output(&gpu, &pipeline)?, 130);
    assert_eq!(compute_output(&gpu, &doubled)?, 260);
    assert_eq!(ray_output(&gpu, &table)?, 130);
    assert_eq!(raster_output(&gpu, &raster)?, 130);
    ensure!(
        instance.validation_errors().is_empty(),
        "reload validation errors: {:?}",
        instance.validation_errors()
    );
    Ok(())
}

fn ray_output(gpu: &Arc<Gpu>, table: &Arc<crate::ShaderBindingTable>) -> Result<u32> {
    let output = gpu.allocate(4, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(gpu, 64)?;
    let root = arguments.push(&output.address())?;
    let mut commands = gpu.commands()?;
    unsafe {
        commands.trace_rays(table, &root, [1, 1, 1], &[output.whole()], &[])?;
    }
    commands.barrier(Access::RAY_TRACING_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut bytes = [0; 4];
    output.read(0, &mut bytes)?;
    Ok(u32::from_ne_bytes(bytes))
}

fn raster_output(gpu: &Arc<Gpu>, pipeline: &Arc<RasterPipeline>) -> Result<u8> {
    let image = gpu.texture(TextureDesc::color(1, 1, vk::Format::R8G8B8A8_UNORM))?;
    let view = image.full_view()?;
    let readback = gpu.allocate(4, MemoryDomain::Readback)?;
    let mut arguments = Arguments::new(gpu, 64)?;
    let root: MemorySlice = arguments.push(&0u64)?;
    let mut commands = gpu.commands()?;
    unsafe {
        commands.initialize(&image)?;
    }
    commands.begin_rendering(
        &[Attachment {
            view: &view,
            clear: Some([0.0; 4]),
            store: true,
            resolve: None,
        }],
        None,
        vk::Extent2D {
            width: 1,
            height: 1,
        },
    )?;
    unsafe {
        commands.draw(pipeline, &root, &root, 0..3, 0..1, &[])?;
    }
    commands.end_rendering()?;
    commands.barrier(
        Access {
            stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        },
        Access::COPY_READ,
    )?;
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: 1,
            height: 1,
            depth: 1,
        });
    unsafe {
        commands.read_image(&image, &readback.whole(), &[region])?;
    }
    commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut bytes = [0; 4];
    readback.read(0, &mut bytes)?;
    Ok(bytes[0])
}

#[cfg(windows)]
#[test]
#[ignore = "opens and verifies a native Windows error dialog"]
fn error_dialog_displays_diagnostics() -> Result<()> {
    use std::ffi::c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, caption: *const u16) -> *mut c_void;
        fn GetWindowThreadProcessId(window: *mut c_void, process: *mut u32) -> u32;
        fn GetDlgItem(window: *mut c_void, id: i32) -> *mut c_void;
        fn EnumChildWindows(
            window: *mut c_void,
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            parameter: isize,
        ) -> i32;
        fn GetWindowTextW(window: *mut c_void, text: *mut u16, limit: i32) -> i32;
        fn PostMessageW(window: *mut c_void, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    let caption = format!("Zenith shader error test {}", std::process::id());
    let title = caption.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let message = "values.slang(3): error 30015: undefined identifier missing_identifier";
    let dialog = std::thread::spawn(move || native_error_window(message, &caption));
    let mut window = std::ptr::null_mut();
    wait_until(|| {
        window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        !window.is_null()
    })?;
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut process);
    }
    assert_eq!(process, std::process::id());
    unsafe extern "system" fn collect(window: *mut c_void, parameter: isize) -> i32 {
        let output = unsafe { &mut *(parameter as *mut Vec<*mut c_void>) };
        output.push(window);
        1
    }
    let visible = wait_until(|| {
        let mut controls = Vec::<*mut c_void>::new();
        unsafe {
            EnumChildWindows(
                window,
                collect,
                &mut controls as *mut Vec<*mut c_void> as isize,
            );
        }
        let mut text = String::new();
        for control in controls {
            let mut buffer = [0u16; 2048];
            let length =
                unsafe { GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32) };
            text.push_str(&String::from_utf16_lossy(&buffer[..length as usize]));
        }
        text.contains("missing_identifier")
    });
    const BM_CLICK: u32 = 0x00F5;
    unsafe {
        let first = GetDlgItem(window, 1);
        let button = if first.is_null() {
            GetDlgItem(window, 2)
        } else {
            first
        };
        PostMessageW(button, BM_CLICK, 0, 0);
    }
    wait_until(|| dialog.is_finished())?;
    dialog.join().unwrap();
    visible?;
    Ok(())
}
