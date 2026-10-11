use crate::pipeline_cache::PipelineKey;
use crate::ray_tracing::binding_table::TableVersion;
use crate::{
    Blend, ComputePipeline, Gpu, RasterDesc, RasterPipeline, RayTracingDesc, RayTracingPipeline,
    RayTracingShaderGroup, Shader, ShaderStage, vk,
};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{CString, OsString},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use zenith_core::log;

const POLL: Duration = Duration::from_millis(250);
const DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CompileRequest {
    program: PathBuf,
    args: Vec<OsString>,
    source: PathBuf,
    entry: CString,
    stage: ShaderStage,
    strides: (u64, u64),
    cache: Option<PathBuf>,
    directory: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

impl CompileRequest {
    pub fn new(
        command: &Command,
        source: &Path,
        entry: CString,
        stage: ShaderStage,
        strides: (u64, u64),
        cache: Option<PathBuf>,
    ) -> Self {
        let mut environment = std::env::vars_os()
            .filter(|(name, _)| name.to_string_lossy().starts_with("SLANG_"))
            .collect::<Vec<_>>();
        environment.sort();
        let directory = std::env::current_dir().unwrap_or_default();
        let cache = cache.map(|path| {
            if path.is_absolute() {
                path
            } else {
                directory.join(path)
            }
        });
        Self {
            program: command.get_program().into(),
            args: command.get_args().map(OsString::from).collect(),
            source: source.to_owned(),
            entry,
            stage,
            strides,
            cache,
            directory,
            environment,
        }
    }

    fn compile(&self) -> Result<Shader> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.directory)
            .envs(self.environment.iter().cloned());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let compiled =
            crate::shader::compile_cached(&mut command, &self.source, self.cache.as_deref())?;
        if !compiled.diagnostics.trim().is_empty() {
            log::warn!(
                "Slang {} ({}):\n{}",
                self.source.display(),
                self.entry.to_string_lossy(),
                compiled.diagnostics
            );
        }
        let code = ash::util::read_spv(&mut std::io::Cursor::new(&compiled.bytes))?;
        Ok(Shader {
            code,
            stage: self.stage,
            entry: self.entry.clone(),
            heap_strides: self.strides,
            request: Some(Arc::new(self.clone())),
            dependencies: compiled.dependencies,
            source: None,
            initial_watch: None,
        })
    }
}

pub(crate) struct Source {
    request: Arc<CompileRequest>,
    pub current: parking_lot::RwLock<Shader>,
    initial: Watch,
    _owner: Arc<Reload>,
}

impl Source {
    fn id(self: &Arc<Self>) -> usize {
        Arc::as_ptr(self) as usize
    }
}

impl Shader {
    pub(crate) fn snapshot(&self) -> Self {
        self.source
            .as_ref()
            .map_or_else(|| self.clone(), |source| source.current.read().clone())
    }
}

pub(crate) struct PipelineVersion {
    pub gpu: Arc<Gpu>,
    pub raw: vk::Pipeline,
}

impl Drop for PipelineVersion {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_pipeline(self.raw, None);
        }
    }
}

#[derive(Clone)]
pub(crate) enum PipelineRecipe {
    Compute {
        shader: Shader,
        specialization: Vec<(u32, u32)>,
    },
    Raster {
        vertex: Shader,
        fragment: Shader,
        vertex_specialization: Vec<(u32, u32)>,
        fragment_specialization: Vec<(u32, u32)>,
        colors: Vec<vk::Format>,
        depth: vk::Format,
        stencil: vk::Format,
        samples: vk::SampleCountFlags,
        topology: vk::PrimitiveTopology,
        blend: Vec<Blend>,
        dynamic_blend: bool,
    },
    RayTracing {
        shaders: Vec<Shader>,
        groups: Vec<RayTracingShaderGroup>,
        max_recursion_depth: u32,
    },
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct LiveKey {
    pub pipeline: PipelineKey,
    sources: Vec<usize>,
    entries: Vec<CString>,
}

impl PipelineRecipe {
    fn shaders(&self) -> Vec<&Shader> {
        match self {
            Self::Compute { shader, .. } => vec![shader],
            Self::Raster {
                vertex, fragment, ..
            } => vec![vertex, fragment],
            Self::RayTracing { shaders, .. } => shaders.iter().collect(),
        }
    }

    fn sources(&self) -> Vec<Arc<Source>> {
        self.shaders()
            .iter()
            .filter_map(|shader| shader.source.clone())
            .collect()
    }

    pub fn key(&self) -> LiveKey {
        let shaders = self
            .shaders()
            .iter()
            .map(|shader| shader.snapshot())
            .collect::<Vec<_>>();
        let pipeline = match self {
            Self::Compute { specialization, .. } => PipelineKey::Compute {
                code: shaders[0].code.clone(),
                specialization: specialization.clone(),
            },
            Self::Raster {
                vertex_specialization,
                fragment_specialization,
                colors,
                depth,
                stencil,
                samples,
                topology,
                blend,
                dynamic_blend,
                ..
            } => PipelineKey::Raster {
                vertex: shaders[0].code.clone(),
                fragment: shaders[1].code.clone(),
                vertex_specialization: vertex_specialization.clone(),
                fragment_specialization: fragment_specialization.clone(),
                colors: colors.clone(),
                depth: *depth,
                stencil: *stencil,
                samples: *samples,
                topology: *topology,
                blend: if *dynamic_blend {
                    Vec::new()
                } else {
                    blend.clone()
                },
                dynamic_blend: *dynamic_blend,
            },
            Self::RayTracing {
                groups,
                max_recursion_depth,
                ..
            } => PipelineKey::RayTracing {
                shaders: shaders
                    .iter()
                    .map(|shader| (shader.stage, shader.entry.clone(), shader.code.clone()))
                    .collect(),
                groups: groups.clone(),
                max_recursion_depth: *max_recursion_depth,
            },
        };
        LiveKey {
            pipeline,
            sources: self
                .shaders()
                .iter()
                .map(|shader| shader.source.as_ref().map_or(0, Source::id))
                .collect(),
            entries: shaders.iter().map(|shader| shader.entry.clone()).collect(),
        }
    }

    pub fn build(
        &self,
        gpu: &Arc<Gpu>,
        changed: &HashMap<usize, Shader>,
    ) -> Result<Arc<PipelineVersion>> {
        let shaders = self
            .shaders()
            .iter()
            .map(|shader| {
                shader
                    .source
                    .as_ref()
                    .and_then(|source| changed.get(&source.id()))
                    .cloned()
                    .unwrap_or_else(|| shader.snapshot())
            })
            .collect::<Vec<_>>();
        match self {
            Self::Compute { specialization, .. } => {
                gpu.compute_version(&shaders[0], specialization)
            }
            Self::Raster {
                vertex_specialization,
                fragment_specialization,
                colors,
                depth,
                stencil,
                samples,
                topology,
                blend,
                dynamic_blend,
                ..
            } => gpu.raster_version(
                &RasterDesc {
                    vertex: &shaders[0],
                    fragment: &shaders[1],
                    colors,
                    depth: *depth,
                    stencil: *stencil,
                    samples: *samples,
                    topology: *topology,
                    blend,
                    dynamic_blend: *dynamic_blend,
                },
                vertex_specialization,
                fragment_specialization,
            ),
            Self::RayTracing {
                groups,
                max_recursion_depth,
                ..
            } => gpu.ray_tracing_version(&RayTracingDesc {
                shaders: &shaders.iter().collect::<Vec<_>>(),
                groups,
                max_recursion_depth: *max_recursion_depth,
            }),
        }
    }
}

#[derive(Clone)]
pub(crate) enum Target {
    Compute(Weak<ComputePipeline>),
    Raster(Weak<RasterPipeline>),
    RayTracing(Weak<RayTracingPipeline>),
    Table(Weak<crate::ShaderBindingTable>),
}

impl Target {
    fn sources(&self) -> Option<Vec<Arc<Source>>> {
        match self {
            Self::Compute(weak) => Some(weak.upgrade()?.recipe.sources()),
            Self::Raster(weak) => Some(weak.upgrade()?.recipe.sources()),
            Self::RayTracing(weak) => Some(weak.upgrade()?.recipe.sources()),
            Self::Table(weak) => Some(weak.upgrade()?.recipe.pipeline.recipe.sources()),
        }
    }

    fn prepare(
        &self,
        shaders: &HashMap<usize, Shader>,
        pipelines: &mut HashMap<usize, Arc<PipelineVersion>>,
    ) -> Result<Option<Update>> {
        Ok(Some(match self {
            Self::Compute(weak) => {
                let Some(target) = weak.upgrade() else {
                    return Ok(None);
                };
                Update::Compute(weak.clone(), target.recipe.build(&target.gpu, shaders)?)
            }
            Self::Raster(weak) => {
                let Some(target) = weak.upgrade() else {
                    return Ok(None);
                };
                Update::Raster(weak.clone(), target.recipe.build(&target.gpu, shaders)?)
            }
            Self::RayTracing(weak) => {
                let Some(target) = weak.upgrade() else {
                    return Ok(None);
                };
                let version = target.recipe.build(&target.gpu, shaders)?;
                pipelines.insert(Arc::as_ptr(&target) as usize, version.clone());
                Update::RayTracing(weak.clone(), version)
            }
            Self::Table(weak) => {
                let Some(target) = weak.upgrade() else {
                    return Ok(None);
                };
                let pipeline = &target.recipe.pipeline;
                let version = pipelines
                    .get(&(Arc::as_ptr(pipeline) as usize))
                    .cloned()
                    .unwrap_or_else(|| pipeline.current.read().clone());
                Update::Table(weak.clone(), target.recipe.build(&version)?)
            }
        }))
    }
}

enum Update {
    Compute(Weak<ComputePipeline>, Arc<PipelineVersion>),
    Raster(Weak<RasterPipeline>, Arc<PipelineVersion>),
    RayTracing(Weak<RayTracingPipeline>, Arc<PipelineVersion>),
    Table(Weak<crate::ShaderBindingTable>, Arc<TableVersion>),
}

impl Update {
    fn apply(self, generation: u64) {
        use std::sync::atomic::Ordering;
        match self {
            Self::Compute(target, version) => {
                if let Some(target) = target.upgrade() {
                    *target.current.write() = version;
                    target.generation.store(generation, Ordering::Release);
                }
            }
            Self::Raster(target, version) => {
                if let Some(target) = target.upgrade() {
                    *target.current.write() = version;
                    target.generation.store(generation, Ordering::Release);
                }
            }
            Self::RayTracing(target, version) => {
                if let Some(target) = target.upgrade() {
                    *target.current.write() = version;
                    target.generation.store(generation, Ordering::Release);
                }
            }
            Self::Table(target, version) => {
                if let Some(target) = target.upgrade() {
                    *target.current.write() = version;
                    target.generation.store(generation, Ordering::Release);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShaderReloadError {
    pub revision: u64,
    pub message: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Watch {
    inputs: BTreeMap<PathBuf, Option<blake3::Hash>>,
    tree: BTreeMap<PathBuf, Option<blake3::Hash>>,
}

#[derive(Default)]
struct Scan {
    files: HashMap<PathBuf, Option<blake3::Hash>>,
    trees: HashMap<PathBuf, BTreeMap<PathBuf, Option<blake3::Hash>>>,
}

impl Scan {
    fn hash(&mut self, path: &Path) -> Option<blake3::Hash> {
        *self
            .files
            .entry(path.to_owned())
            .or_insert_with(|| std::fs::read(path).ok().map(|bytes| blake3::hash(&bytes)))
    }

    fn tree(&mut self, root: &Path) -> BTreeMap<PathBuf, Option<blake3::Hash>> {
        let normalized = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let root = normalized.as_path();
        if let Some(tree) = self.trees.get(root) {
            return tree.clone();
        }
        let mut tree = BTreeMap::new();
        let mut directories = vec![root.to_owned()];
        while let Some(directory) = directories.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                tree.insert(directory, None);
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    directories.push(path);
                } else if kind.is_file()
                    && path.extension().is_some_and(|extension| {
                        matches!(
                            extension.to_string_lossy().to_ascii_lowercase().as_str(),
                            "slang" | "slang-module" | "h" | "hlsl"
                        )
                    })
                {
                    tree.insert(path.clone(), self.hash(&path));
                }
            }
        }
        self.trees.insert(root.to_owned(), tree.clone());
        tree
    }
}

impl Watch {
    pub(crate) fn read(request: &CompileRequest, dependencies: &[PathBuf]) -> Self {
        Self::scan(request, dependencies, &mut Scan::default())
    }

    fn scan(request: &CompileRequest, dependencies: &[PathBuf], scan: &mut Scan) -> Self {
        let inputs = dependencies
            .iter()
            .chain(std::iter::once(&request.source))
            .map(|path| (path.clone(), scan.hash(path)))
            .collect();
        let mut tree = scan.tree(request.source.parent().unwrap());
        tree.extend(scan.tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders")));
        Self { inputs, tree }
    }

    fn changed(&self, previous: &Self, failed: bool) -> bool {
        self.inputs != previous.inputs
            || self.tree.keys().ne(previous.tree.keys())
            || (failed && self.tree != previous.tree)
    }

    pub(crate) fn compiled(
        mut self,
        dependencies: &[PathBuf],
        started: std::time::SystemTime,
    ) -> Self {
        for path in dependencies {
            let hash = self
                .inputs
                .get(path)
                .or_else(|| self.tree.get(path))
                .copied()
                .unwrap_or_else(|| {
                    if std::fs::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .is_ok_and(|modified| modified > started)
                    {
                        None
                    } else {
                        std::fs::read(path).ok().map(|bytes| blake3::hash(&bytes))
                    }
                });
            self.inputs.insert(path.clone(), hash);
        }
        self
    }
}

struct Batch {
    sources: Vec<(Weak<Source>, Shader)>,
    updates: Vec<Update>,
    watches: Vec<Watched>,
    registration: u64,
}

struct Watched {
    source: Weak<Source>,
    dependencies: Vec<PathBuf>,
    watch: Watch,
}

#[derive(Default)]
struct State {
    sources: HashMap<(Arc<CompileRequest>, blake3::Hash), Weak<Source>>,
    targets: Vec<Target>,
    ready: Option<Batch>,
    registration: u64,
    generation: u64,
    retry: HashSet<usize>,
    installed: HashMap<usize, Watch>,
    error: Option<ShaderReloadError>,
    revision: u64,
}

#[derive(Default)]
pub(crate) struct Reload {
    state: Mutex<State>,
    dialog: Arc<Mutex<Dialog>>,
    active: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Default)]
struct Dialog {
    open: bool,
    pending: Option<String>,
}

impl Reload {
    pub fn new(active: Arc<std::sync::atomic::AtomicUsize>) -> Result<Arc<Self>> {
        let reload = Arc::new(Self {
            active,
            ..Self::default()
        });
        let weak = Arc::downgrade(&reload);
        std::thread::Builder::new()
            .name("zenith-shader-reload".into())
            .spawn(move || worker(weak))
            .context("start shader reload worker")?;
        Ok(reload)
    }

    pub fn track_shader(self: &Arc<Self>, mut shader: Shader) -> Result<Shader> {
        let initial = shader.initial_watch.take();
        let request = shader
            .request
            .as_ref()
            .context("shader has no compilation recipe")?
            .clone();
        let key = (
            request.clone(),
            blake3::hash(bytemuck::cast_slice(&shader.code)),
        );
        let mut state = self.state.lock();
        let source = state
            .sources
            .get(&key)
            .and_then(Weak::upgrade)
            .filter(|source| source.current.read().code == shader.code)
            .unwrap_or_else(|| {
                let source = Arc::new(Source {
                    initial: initial.unwrap_or_else(|| Watch::read(&request, &shader.dependencies)),
                    request,
                    current: parking_lot::RwLock::new(shader.clone()),
                    _owner: self.clone(),
                });
                state.sources.insert(key, Arc::downgrade(&source));
                state.registration += 1;
                source
            });
        shader.source = Some(source);
        Ok(shader)
    }

    pub fn track(&self, target: Target) {
        let mut state = self.state.lock();
        state.targets.push(target);
        state.registration += 1;
    }

    fn begin(self: &Arc<Self>) -> ShaderReloadScope {
        let mut state = self.state.lock();
        if self.active.load(std::sync::atomic::Ordering::Acquire) == 0
            && let Some(batch) = state.ready.take()
        {
            let mut scan = Scan::default();
            if batch.registration == state.registration
                && batch.watches.iter().all(|watched| {
                    watched.source.upgrade().is_none_or(|source| {
                        Watch::scan(&source.request, &watched.dependencies, &mut scan)
                            == watched.watch
                    })
                })
            {
                state.generation += 1;
                let generation = state.generation;
                for (source, shader) in batch.sources {
                    if let Some(source) = source.upgrade() {
                        if let Some(watched) = batch.watches.iter().find(|watched| {
                            watched
                                .source
                                .upgrade()
                                .is_some_and(|candidate| Arc::ptr_eq(&source, &candidate))
                        }) {
                            state.installed.insert(source.id(), watched.watch.clone());
                        }
                        *source.current.write() = shader;
                    }
                }
                for update in batch.updates {
                    update.apply(generation);
                }
                state.error = None;
                self.dialog.lock().pending = None;
                log::info!("Shader reload installed (generation {generation})");
            } else {
                state.retry.extend(
                    batch
                        .sources
                        .iter()
                        .filter_map(|(source, _)| source.upgrade().map(|source| source.id())),
                );
            }
        }
        self.active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        ShaderReloadScope {
            _reload: Some(self.clone()),
            active: self.active.clone(),
        }
    }

    pub(crate) fn failure(&self, message: String) {
        log::error!("{message}");
        let mut state = self.state.lock();
        state.revision += 1;
        state.error = Some(ShaderReloadError {
            revision: state.revision,
            message: message.clone(),
        });
        drop(state);
        if !cfg!(test) {
            self.show_error(message);
        }
    }

    fn show_error(&self, message: String) {
        #[cfg(windows)]
        {
            let mut dialog = self.dialog.lock();
            dialog.pending = Some(message);
            if dialog.open {
                return;
            }
            dialog.open = true;
            let shared = self.dialog.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("zenith-shader-error".into())
                .spawn(move || {
                    loop {
                        let message = {
                            let mut dialog = shared.lock();
                            let Some(message) = dialog.pending.take() else {
                                dialog.open = false;
                                break;
                            };
                            message
                        };
                        error_window(&message);
                    }
                })
            {
                dialog.open = false;
                log::error!("Cannot open shader error window: {error}");
            }
        }
        #[cfg(not(windows))]
        {
            let _ = message;
        }
    }
}

pub struct ShaderReloadScope {
    _reload: Option<Arc<Reload>>,
    active: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for ShaderReloadScope {
    fn drop(&mut self) {
        self.active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

impl Gpu {
    pub(crate) fn reload_service(&self) -> Arc<Reload> {
        let mut weak = self.shader_reload.lock();
        if let Some(reload) = weak.upgrade() {
            return reload;
        }
        let reload = Reload::new(self.shader_recordings.clone()).unwrap_or_else(|error| {
            log::error!("{error:#}");
            Arc::new(Reload {
                active: self.shader_recordings.clone(),
                ..Reload::default()
            })
        });
        *weak = Arc::downgrade(&reload);
        reload
    }

    pub fn shader_reload_scope(&self) -> ShaderReloadScope {
        match self.shader_reload.lock().upgrade() {
            Some(reload) => reload.begin(),
            None => {
                self.shader_recordings
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                ShaderReloadScope {
                    _reload: None,
                    active: self.shader_recordings.clone(),
                }
            }
        }
    }

    pub fn shader_reload_error(&self) -> Option<ShaderReloadError> {
        self.shader_reload
            .lock()
            .upgrade()?
            .state
            .lock()
            .error
            .clone()
    }

    pub fn shader_reload_generation(&self) -> u64 {
        self.shader_reload
            .lock()
            .upgrade()
            .map_or(0, |reload| reload.state.lock().generation)
    }
}

struct Observed {
    watch: Watch,
    changed: Option<Instant>,
    failed: bool,
    dirty: bool,
}

fn worker(weak: Weak<Reload>) {
    let mut observed = HashMap::<usize, Observed>::new();
    loop {
        std::thread::sleep(POLL);
        let Some(reload) = weak.upgrade() else { return };
        let (sources, targets, registered, retry, installed) = {
            let mut state = reload.state.lock();
            state.sources.retain(|_, source| source.strong_count() > 0);
            state.targets.retain(|target| target.sources().is_some());
            (
                state
                    .sources
                    .values()
                    .filter_map(Weak::upgrade)
                    .collect::<Vec<_>>(),
                state.targets.clone(),
                state.registration,
                std::mem::take(&mut state.retry),
                std::mem::take(&mut state.installed),
            )
        };
        observed.retain(|id, _| sources.iter().any(|source| source.id() == *id));
        let mut changed = Vec::new();
        let mut event = !retry.is_empty();
        let mut scan = Scan::default();
        for source in &sources {
            let watch = Watch::scan(
                &source.request,
                &source.current.read().dependencies,
                &mut scan,
            );
            let observation = observed.entry(source.id()).or_insert_with(|| Observed {
                watch: source.initial.clone(),
                changed: None,
                failed: false,
                dirty: false,
            });
            if let Some(installed_watch) = installed.get(&source.id()) {
                observation.watch = installed_watch.clone();
                observation.dirty = false;
                observation.failed = false;
            }
            if retry.contains(&source.id()) {
                observation.dirty = true;
            }
            if watch.changed(&observation.watch, observation.failed) {
                observation.dirty = true;
                event = true;
                let mut state = reload.state.lock();
                if let Some(batch) = state.ready.take() {
                    state.retry.extend(
                        batch
                            .sources
                            .iter()
                            .filter_map(|(source, _)| source.upgrade().map(|source| source.id())),
                    );
                }
            }
            observation.watch = watch;
        }
        if event {
            for observation in observed
                .values_mut()
                .filter(|observation| observation.dirty)
            {
                observation.changed = Some(Instant::now());
            }
        }
        for source in &sources {
            if observed[&source.id()]
                .changed
                .is_some_and(|time| time.elapsed() >= DEBOUNCE)
            {
                changed.push(source.clone());
            }
        }
        if changed.is_empty() {
            continue;
        }
        let mut watches = sources
            .iter()
            .map(|source| Watched {
                source: Arc::downgrade(source),
                dependencies: source.current.read().dependencies.clone(),
                watch: observed[&source.id()].watch.clone(),
            })
            .collect::<Vec<_>>();
        let mut shaders = HashMap::new();
        let mut failure = None;
        for source in &changed {
            match source.request.compile() {
                Ok(shader) => {
                    shaders.insert(source.id(), shader);
                }
                Err(error) => {
                    failure = Some(format!(
                        "Shader reload failed: {} ({}, {:?})\n{error:#}\n\nRendering continues with the last successful version.",
                        source.request.source.display(),
                        source.request.entry.to_string_lossy(),
                        source.request.stage
                    ));
                    break;
                }
            }
        }
        let mut updates = Vec::new();
        if failure.is_none() {
            let mut pipelines = HashMap::new();
            let mut affected = targets
                .iter()
                .filter(|target| {
                    target.sources().is_some_and(|sources| {
                        sources
                            .iter()
                            .any(|source| shaders.contains_key(&source.id()))
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            affected.sort_by_key(|target| matches!(target, Target::Table(_)));
            for target in affected {
                match target.prepare(&shaders, &mut pipelines) {
                    Ok(Some(update)) => updates.push(update),
                    Ok(None) => {}
                    Err(error) => {
                        failure = Some(format!(
                            "Shader pipeline reload failed:\n{error:#}\n\nRendering continues with the last successful version."
                        ));
                        break;
                    }
                }
            }
        }
        let mut scan = Scan::default();
        if watches.iter().any(|watched| {
            watched.source.upgrade().is_some_and(|source| {
                Watch::scan(&source.request, &watched.dependencies, &mut scan) != watched.watch
            })
        }) || reload.state.lock().registration != registered
        {
            for source in &changed {
                observed.get_mut(&source.id()).unwrap().changed = Some(Instant::now());
            }
            continue;
        }
        for source in &changed {
            let observation = observed.get_mut(&source.id()).unwrap();
            observation.changed = None;
            observation.failed = failure.is_some();
        }
        if let Some(message) = failure {
            reload.failure(message);
        } else {
            for watched in &mut watches {
                if let Some(source) = watched.source.upgrade()
                    && let Some(shader) = shaders.get(&source.id())
                {
                    watched.dependencies = shader.dependencies.clone();
                    watched.watch = Watch::read(&source.request, &watched.dependencies);
                }
            }
            let batch = Batch {
                sources: changed
                    .iter()
                    .map(|source| {
                        (
                            Arc::downgrade(source),
                            shaders.remove(&source.id()).unwrap(),
                        )
                    })
                    .collect(),
                updates,
                watches,
                registration: registered,
            };
            reload.state.lock().ready = Some(batch);
        }
    }
}

#[cfg(windows)]
fn error_window(message: &str) {
    native_error_window(message, "Zenith — Shader compilation error");
}

#[cfg(windows)]
fn native_error_window(message: &str, caption: &str) {
    const MB_ICONERROR: u32 = 0x00000010;
    const MB_SETFOREGROUND: u32 = 0x00010000;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            flags: u32,
        ) -> i32;
    }
    let text = message
        .replace('\0', "")
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let caption = caption.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONERROR | MB_SETFOREGROUND,
        )
    };
    if result == 0 {
        log::error!(
            "Cannot display shader error window: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests;
