use super::*;
use std::sync::{Arc, atomic::AtomicUsize};

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    compiler: PathBuf,
    cache: PathBuf,
}
impl Fixture {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "zenith-shader-test-{}-{}",
            std::process::id(),
            TEMP_IDS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("source space"))?;
        fs::create_dir_all(root.join("bin"))?;
        let source = root.join("source space/main.slang");
        let compiler = root.join("bin/compiler.exe");
        fs::write(&source, "original")?;
        fs::write(&compiler, "compiler")?;
        Ok(Self {
            source: source.canonicalize()?,
            compiler,
            cache: root.join("cache"),
            root,
        })
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.compiler);
        command
            .arg(&self.source)
            .args(["-entry", "main", "-stage", "compute"]);
        command
    }
    fn store(&self, cache: &Cache) -> Result<()> {
        let depfile = cache.depfile()?;
        fs::write(&depfile.0, "-:\n")?;
        cache.store(
            &Compiled {
                bytes: code(),
                diagnostics: "test diagnostic".into(),
            },
            &depfile.0,
            SystemTime::now(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.parent() == Some(std::env::temp_dir().as_path())
            && self
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("zenith-shader-test-")
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn code() -> Vec<u8> {
    [0x0723_0203_u32, 0x0001_0600, 0, 1, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

#[test]
fn parses_make_dependencies_with_windows_paths_escaping_and_continuations() -> Result<()> {
    let paths = dependencies(
        "C:\\out.spv: C\\:\\\\shader\\ space\\\\main.slang \\\r\n dir/file\\#name\\:part.slang dollar$$file.slang\n",
    )?;
    assert_eq!(
        paths,
        BTreeSet::from([
            PathBuf::from("C:\\shader space\\main.slang"),
            PathBuf::from("dir/file#name:part.slang"),
            PathBuf::from("dollar$file.slang"),
        ])
    );
    assert!(dependencies("no target").is_err());
    assert!(dependencies("-: trailing\\").is_err());
    Ok(())
}

#[test]
fn cache_tracks_content_options_compiler_and_directory_changes() -> Result<()> {
    let fixture = Fixture::new()?;
    let unrelated = fixture.source.parent().unwrap().join("unrelated.slang");
    fs::write(&unrelated, "original")?;
    let original_path = {
        let cache = Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?;
        fixture.store(&cache)?;
        assert_eq!(cache.load()?.bytes, code());
        assert_eq!(cache.load()?.diagnostics, "test diagnostic");
        cache.path.clone()
    };
    fs::write(&unrelated, "an unrelated shader edit")?;
    assert_eq!(
        Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?
            .load()?
            .bytes,
        code()
    );
    let modified = fs::metadata(&fixture.source)?.modified()?;
    fs::write(&fixture.source, "modified")?;
    File::options()
        .write(true)
        .open(&fixture.source)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    {
        let cache = Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?;
        assert!(cache.load().is_err());
        fixture.store(&cache)?;
    }
    fs::create_dir(fixture.source.parent().unwrap().join("new_import"))?;
    {
        let cache = Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?;
        assert!(cache.load().is_err());
        fixture.store(&cache)?;
    }
    for arguments in [
        &["-entry", "other"][..],
        &["-stage", "vertex"],
        &["-target", "spirv"],
        &["-profile", "spirv_1_6"],
        &["-capability", "spvDescriptorHeapEXT"],
        &["-I", "other/include"],
        &["-g3"],
        &["-O3"],
        &["-spirv-resource-heap-stride", "64"],
        &["-spirv-sampler-heap-stride", "64"],
    ] {
        let mut changed = fixture.command();
        changed.args(arguments);
        assert_ne!(
            Cache::open(&fixture.cache, &changed, &fixture.source)?.path,
            original_path
        );
    }
    let runtime = fixture.compiler.parent().unwrap().join("slang.dll");
    fs::write(&runtime, "original runtime")?;
    let runtime_path = Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?
        .path
        .clone();
    assert_ne!(runtime_path, original_path);
    fs::write(runtime, "replacement runtime")?;
    assert_ne!(
        Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?.path,
        runtime_path
    );
    fs::write(&fixture.compiler, "replacement compiler")?;
    assert_ne!(
        Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?.path,
        original_path
    );
    Ok(())
}

#[test]
fn corrupt_incomplete_and_changed_during_compile_results_are_rejected() -> Result<()> {
    let fixture = Fixture::new()?;
    let cache = Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?;
    fixture.store(&cache)?;
    let original = fs::read(&cache.path)?;
    for bytes in [
        vec![],
        u64::MAX.to_le_bytes().to_vec(),
        original[..12].to_vec(),
        {
            let mut corrupt = original.clone();
            *corrupt.last_mut().unwrap() ^= 1;
            corrupt
        },
    ] {
        fs::write(&cache.path, bytes)?;
        assert!(cache.load().is_err());
    }
    fs::write(&cache.path, original)?;
    let temporary = Temporary::new(cache.path.parent().unwrap(), "interrupted")?;
    fs::write(&temporary.0, "partial")?;
    assert_eq!(cache.load()?.bytes, code());
    let depfile = cache.depfile()?;
    fs::write(&depfile.0, "-:\n")?;
    assert!(
        cache
            .store(
                &Compiled {
                    bytes: code(),
                    diagnostics: String::new()
                },
                &depfile.0,
                UNIX_EPOCH
            )
            .is_err()
    );
    assert_eq!(cache.load()?.bytes, code());
    Ok(())
}

#[test]
fn simultaneous_requests_compile_once() -> Result<()> {
    let fixture = Fixture::new()?;
    let compiles = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let jobs = (0..8)
            .map(|_| {
                scope.spawn(|| -> Result<()> {
                    let cache = Cache::open(&fixture.cache, &fixture.command(), &fixture.source)?;
                    if cache.load().is_err() {
                        compiles.fetch_add(1, Ordering::Relaxed);
                        fixture.store(&cache)?;
                    }
                    assert_eq!(cache.load()?.bytes, code());
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        for job in jobs {
            job.join().unwrap()?;
        }
        Ok::<_, anyhow::Error>(())
    })?;
    assert_eq!(compiles.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn cache_child() -> Result<()> {
    let Some(root) = std::env::var_os("ZENITH_SHADER_TEST_ROOT") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    let source = root.join("source space/main.slang").canonicalize()?;
    let mut command = Command::new(root.join("bin/compiler.exe"));
    command.arg(&source);
    let cache = Cache::open(&root.join("cache"), &command, &source)?;
    if cache.load().is_err() {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("compiles"))?
            .write_all(b"1")?;
        let depfile = cache.depfile()?;
        fs::write(&depfile.0, "-:\n")?;
        cache.store(
            &Compiled {
                bytes: code(),
                diagnostics: String::new(),
            },
            &depfile.0,
            SystemTime::now(),
        )?;
    }
    Ok(())
}

#[test]
fn processes_share_an_atomic_cache() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut children = Vec::new();
    for _ in 0..3 {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--exact", "shader_cache::tests::cache_child"])
            .env("ZENITH_SHADER_TEST_ROOT", &fixture.root);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        children.push(command.stdout(std::process::Stdio::null()).spawn()?);
    }
    for mut child in children {
        assert!(child.wait()?.success());
    }
    assert_eq!(fs::read(fixture.root.join("compiles"))?, b"1");
    Ok(())
}

#[test]
#[ignore = "requires the Slang SDK"]
fn slang_cache_tracks_imports_and_recovers_without_hiding_compile_errors() -> Result<()> {
    let fixture = Fixture::new()?;
    let compiler = std::env::var_os("ZENITH_SLANGC")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("SLANG_DIR").map(|directory| {
                PathBuf::from(directory).join("bin").join(if cfg!(windows) {
                    "slangc.exe"
                } else {
                    "slangc"
                })
            })
        })
        .unwrap_or_else(|| "slangc".into());
    let compiler = resolve_compiler(&compiler)?;
    let import = fixture.source.parent().unwrap().join("values.slang");
    fs::write(&import, "public uint value() { return 7; }")?;
    fs::write(
        &fixture.source,
        "import values; RWStructuredBuffer<uint> output; [shader(\"compute\")][numthreads(1,1,1)] void main(uint3 id : SV_DispatchThreadID) { output[id.x] = value(); }",
    )?;
    let command = || {
        let mut command = Command::new(&compiler);
        command
            .arg(&fixture.source)
            .args([
                "-entry", "main", "-stage", "compute", "-target", "spirv", "-O0", "-g3", "-I",
            ])
            .arg(fixture.source.parent().unwrap())
            .args(["-o", "-"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    };
    let compile =
        || crate::shader::compile_cached(&mut command(), &fixture.source, Some(&fixture.cache));
    let first = compile()?;
    let path = {
        let cache = Cache::open(&fixture.cache, &command(), &fixture.source)?;
        assert_eq!(cache.load()?.bytes, first.bytes);
        cache.path.clone()
    };
    assert_eq!(compile()?.bytes, first.bytes);
    let unavailable = fixture.root.join("unavailable");
    fs::write(&unavailable, "not a directory")?;
    assert_eq!(
        crate::shader::compile_cached(&mut command(), &fixture.source, Some(&unavailable))?.bytes,
        first.bytes
    );
    let mut truncated = File::create(&path)?;
    truncated.write_all(b"interrupted")?;
    drop(truncated);
    assert_eq!(compile()?.bytes, first.bytes);
    fs::write(&import, "public uint value() { return 9; }")?;
    let changed = compile()?;
    assert_ne!(changed.bytes, first.bytes);
    fs::write(&import, "this does not compile")?;
    let failed = compile()
        .err()
        .context("invalid import unexpectedly compiled")?
        .to_string();
    assert!(failed.contains("Slang failed") && failed.contains("values.slang"));
    fs::write(&import, "public uint value() { return 9; }")?;
    assert_eq!(compile()?.bytes, changed.bytes);
    Ok(())
}
