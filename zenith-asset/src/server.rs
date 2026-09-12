use crate::{
    Asset, AssetAddress, AssetCodec, AssetError, AssetId, AssetPath, AssetSource, CookedAsset,
    CpuRetention, ErasedValue, ErrorKind, Handle, ImportContext, Importer, Result, Revision,
    SerdeCodec,
    cache::{Cache, Dependency, Manifest, Output},
    handle::{Slot, SlotOps},
    import::{
        CodecAdapter, ErasedCodec, ErasedImporter, ImporterAdapter, MAX_ARTIFACT_BYTES,
        settings_bytes,
    },
};
use parking_lot::{Mutex, RwLock};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::{
    any::TypeId,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

static SERVER_IDS: AtomicU64 = AtomicU64::new(1);
type RequestKey = (AssetAddress, String);
type SlotFactory =
    fn(AssetId, Option<AssetAddress>, Arc<RwLock<()>>, CpuRetention) -> Arc<dyn SlotOps>;

pub(crate) struct RegisteredType {
    pub type_id: TypeId,
    pub key: &'static str,
    pub schema: u32,
    pub codec: Arc<dyn ErasedCodec>,
    slot: SlotFactory,
}
#[derive(Default)]
struct Counters {
    imports: AtomicU64,
    decodes: AtomicU64,
    cache_hits: AtomicU64,
    source_bytes: AtomicU64,
    artifact_bytes: AtomicU64,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct AssetStats {
    pub imports: u64,
    pub decodes: u64,
    pub cache_hits: u64,
    pub source_bytes: u64,
    pub artifact_bytes: u64,
}
#[derive(Debug, Clone)]
pub enum AssetEvent {
    Ready {
        id: AssetId,
        address: AssetAddress,
        revision: Revision,
    },
    Failed {
        id: AssetId,
        address: AssetAddress,
        error: AssetError,
    },
}
#[derive(Clone)]
struct Selection {
    importer: String,
    settings: Vec<u8>,
}
struct Shared {
    id: u64,
    ids: AtomicU64,
    types: BTreeMap<String, Arc<RegisteredType>>,
    retention: HashMap<TypeId, CpuRetention>,
    sources: BTreeMap<String, Arc<dyn AssetSource>>,
    importers: BTreeMap<String, Arc<dyn ErasedImporter>>,
    slots: Mutex<HashMap<(AssetAddress, TypeId), Weak<dyn SlotOps>>>,
    selections: Mutex<BTreeMap<AssetAddress, Selection>>,
    events: Mutex<VecDeque<AssetEvent>>,
    cache: Option<Cache>,
    packaged: bool,
    target: String,
    counters: Counters,
    watch_ms: AtomicU64,
    publication: Arc<RwLock<()>>,
    builds: Mutex<BTreeMap<AssetAddress, Manifest>>,
}
impl Shared {
    fn slot(&self, address: &AssetAddress, ty: &RegisteredType) -> Arc<dyn SlotOps> {
        let mut slots = self.slots.lock();
        let key = (address.clone(), ty.type_id);
        if let Some(slot) = slots.get(&key).and_then(Weak::upgrade) {
            return slot;
        }
        if slots.len() >= 1024 {
            slots.retain(|_, slot| slot.strong_count() > 0);
        }
        let slot = (ty.slot)(
            AssetId {
                server: self.id,
                slot: self.ids.fetch_add(1, Ordering::Relaxed),
            },
            Some(address.clone()),
            self.publication.clone(),
            self.retention.get(&ty.type_id).copied().unwrap_or_default(),
        );
        slots.insert(key, Arc::downgrade(&slot));
        slot
    }
    fn find(&self, address: &AssetAddress, ty: &RegisteredType) -> Option<Arc<dyn SlotOps>> {
        self.slots
            .lock()
            .get(&(address.clone(), ty.type_id))
            .and_then(Weak::upgrade)
    }
    fn event(&self, event: AssetEvent) {
        let mut events = self.events.lock();
        if events.len() == 1024 {
            events.pop_front();
        }
        events.push_back(event);
    }
}
enum Job {
    Load {
        slot: Arc<dyn SlotOps>,
        key: String,
        reload: bool,
    },
    Refresh,
}
struct Runtime {
    sender: Mutex<Option<SyncSender<Job>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.sender.get_mut().take();
        if let Some(worker) = self.worker.get_mut().take() {
            let _ = worker.join();
        }
    }
}
#[derive(Clone)]
pub struct AssetServer {
    shared: Arc<Shared>,
    runtime: Arc<Runtime>,
}

pub struct AssetServerBuilder {
    types: BTreeMap<String, Arc<RegisteredType>>,
    retention: HashMap<TypeId, CpuRetention>,
    sources: BTreeMap<String, Arc<dyn AssetSource>>,
    importers: BTreeMap<String, Arc<dyn ErasedImporter>>,
    cache: Option<PathBuf>,
    packaged: bool,
    target: String,
    error: Option<AssetError>,
}
impl Default for AssetServerBuilder {
    fn default() -> Self {
        Self {
            types: BTreeMap::new(),
            retention: HashMap::new(),
            sources: BTreeMap::new(),
            importers: BTreeMap::new(),
            cache: None,
            packaged: false,
            target: "desktop".into(),
            error: None,
        }
    }
}
impl AssetServerBuilder {
    pub fn source(self, source: impl AssetSource) -> Self {
        self.mount("default", source)
    }
    pub fn mount(mut self, name: &str, source: impl AssetSource) -> Self {
        if AssetAddress::parse(&format!("{name}://validation")).is_err()
            || self.sources.insert(name.into(), Arc::new(source)).is_some()
        {
            self.error = Some(AssetError::new(
                ErrorKind::Registration,
                format!("invalid or duplicate source: {name}"),
            ));
        }
        self
    }
    pub fn cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache = Some(path.into());
        self
    }
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }
    pub fn packaged(mut self) -> Self {
        self.packaged = true;
        self
    }
    pub fn register_asset<T: CookedAsset>(self) -> Self {
        self.register_codec::<T>(SerdeCodec)
    }
    pub fn cpu_retention<T: Asset>(mut self, policy: CpuRetention) -> Self {
        self.retention.insert(TypeId::of::<T>(), policy);
        self
    }
    pub fn register_codec<T: CookedAsset>(mut self, codec: impl AssetCodec<T>) -> Self {
        if T::TYPE_KEY.is_empty()
            || self.types.contains_key(T::TYPE_KEY)
            || self
                .types
                .values()
                .any(|ty| ty.type_id == TypeId::of::<T>())
        {
            self.error = Some(AssetError::new(
                ErrorKind::Registration,
                format!("duplicate or invalid asset type: {}", T::TYPE_KEY),
            ));
            return self;
        }
        self.types.insert(
            T::TYPE_KEY.into(),
            Arc::new(RegisteredType {
                type_id: TypeId::of::<T>(),
                key: T::TYPE_KEY,
                schema: T::SCHEMA_VERSION,
                codec: Arc::new(CodecAdapter::<T, _>(codec, PhantomData)),
                slot: |id, address, publication, retention| {
                    Arc::new(Slot::<T>::new(id, address, publication, retention))
                },
            }),
        );
        self
    }
    pub fn register_importer<I: Importer>(mut self, importer: I) -> Self {
        if I::KEY.is_empty() || self.importers.contains_key(I::KEY) {
            self.error = Some(AssetError::new(
                ErrorKind::Registration,
                format!("duplicate or invalid importer: {}", I::KEY),
            ));
            return self;
        }
        for extension in importer.extensions() {
            if extension.is_empty()
                || *extension != extension.to_ascii_lowercase()
                || self
                    .importers
                    .values()
                    .any(|other| other.extensions().contains(extension))
            {
                self.error = Some(AssetError::new(
                    ErrorKind::Registration,
                    format!("invalid or ambiguous importer extension: {extension}"),
                ));
            }
        }
        self.importers
            .insert(I::KEY.into(), Arc::new(ImporterAdapter(importer)));
        self
    }
    pub fn with_builtin_assets(self) -> Self {
        let builder = self
            .register_asset::<crate::mesh::Mesh>()
            .register_asset::<crate::mesh::Scene>()
            .register_asset::<crate::material::Material>()
            .register_codec::<crate::texture::Texture>(crate::texture_codec::TextureCodec);
        #[cfg(feature = "importers")]
        let builder = builder
            .register_importer(crate::gltf::GltfImporter)
            .register_importer(crate::hdr::HdrImporter)
            .register_importer(crate::texture::ImageImporter);
        builder
    }
    pub fn build(self) -> Result<AssetServer> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.packaged && self.cache.is_none() {
            return Err(AssetError::new(
                ErrorKind::Registration,
                "packaged assets require a cache directory",
            ));
        }
        for importer in self.importers.values() {
            if !self.types.contains_key(importer.output()) {
                return Err(AssetError::new(
                    ErrorKind::Registration,
                    format!("unregistered importer output: {}", importer.output()),
                ));
            }
        }
        let shared = Arc::new(Shared {
            id: SERVER_IDS.fetch_add(1, Ordering::Relaxed),
            ids: AtomicU64::new(1),
            types: self.types,
            retention: self.retention,
            sources: self.sources,
            importers: self.importers,
            slots: Mutex::new(HashMap::new()),
            selections: Mutex::new(BTreeMap::new()),
            events: Mutex::new(VecDeque::new()),
            cache: self.cache.map(|path| Cache::new(path, self.target.clone())),
            packaged: self.packaged,
            target: self.target,
            counters: Counters::default(),
            watch_ms: AtomicU64::new(0),
            publication: Arc::new(RwLock::new(())),
            builds: Mutex::new(BTreeMap::new()),
        });
        #[cfg(feature = "parallel")]
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("zenith-asset-{i}"))
            .build()
            .map_err(|e| AssetError::caused_by(ErrorKind::Registration, "start asset pool", e))?;
        let (sender, receiver) = mpsc::sync_channel(64);
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("zenith-assets".into())
            .spawn(move || {
                loop {
                    let watch = worker_shared.watch_ms.load(Ordering::Relaxed);
                    let result = receiver.recv_timeout(Duration::from_millis(if watch == 0 {
                        1000
                    } else {
                        watch
                    }));
                    let job = match result {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) if watch > 0 => Job::Refresh,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let run = || match job {
                        Job::Load { slot, key, reload } => {
                            process(&worker_shared, slot, key, reload)
                        }
                        Job::Refresh => refresh(&worker_shared),
                    };
                    #[cfg(feature = "parallel")]
                    pool.install(run);
                    #[cfg(not(feature = "parallel"))]
                    run();
                }
            })?;
        Ok(AssetServer {
            shared,
            runtime: Arc::new(Runtime {
                sender: Mutex::new(Some(sender)),
                worker: Mutex::new(Some(worker)),
            }),
        })
    }
}
impl AssetServer {
    pub fn builder() -> AssetServerBuilder {
        AssetServerBuilder::default()
    }
    pub fn load<T: CookedAsset>(&self, path: &str) -> Result<Handle<T>> {
        self.load_path(&AssetPath::new(path)?)
    }
    pub fn load_path<T: CookedAsset>(&self, path: &AssetPath<T>) -> Result<Handle<T>> {
        let ty = self.registered::<T>()?;
        let slot = self.shared.slot(path.address(), &ty);
        if slot.begin(false)
            && let Err(error) = self.send(Job::Load {
                slot: slot.clone(),
                key: ty.key.into(),
                reload: false,
            })
        {
            slot.fail(error.clone());
            return Err(error);
        }
        Handle::from_erased(slot)
    }
    pub fn load_blocking<T: CookedAsset>(&self, path: &str) -> Result<Handle<T>> {
        let handle = self.load(path)?;
        handle.wait()?;
        Ok(handle)
    }
    pub fn load_with<I: Importer>(
        &self,
        path: &str,
        settings: &I::Settings,
    ) -> Result<Handle<I::Output>> {
        let mut address = AssetAddress::parse(path)?;
        let bytes = settings_bytes(settings)?;
        if !self.shared.importers.contains_key(I::KEY) {
            return Err(AssetError::new(
                ErrorKind::Registration,
                format!("importer not registered: {}", I::KEY),
            ));
        }
        let mut digest = blake3::Hasher::new();
        digest.update(I::KEY.as_bytes());
        digest.update(&bytes);
        address.set_variant(digest.finalize().to_hex().to_string());
        self.shared.selections.lock().insert(
            address.root(),
            Selection {
                importer: I::KEY.into(),
                settings: bytes,
            },
        );
        self.load_path(&AssetPath::from_address(address))
    }
    pub fn add<T: Asset>(&self, value: T) -> Handle<T> {
        let slot = Arc::new(Slot::new(
            AssetId {
                server: self.shared.id,
                slot: self.shared.ids.fetch_add(1, Ordering::Relaxed),
            },
            None,
            self.shared.publication.clone(),
            self.shared
                .retention
                .get(&TypeId::of::<T>())
                .copied()
                .unwrap_or_default(),
        ));
        let value: ErasedValue = Arc::new(value);
        slot.publish(value, "generated".into(), Vec::new())
            .expect("generated slot type matches its value");
        Handle { slot }
    }
    pub fn reload<T: CookedAsset>(&self, handle: &Handle<T>) -> Result<()> {
        if handle.id().server != self.shared.id {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                "handle belongs to another asset server",
            ));
        }
        let ty = self.registered::<T>()?;
        if handle.address().is_none() {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                "generated assets have no source to reload",
            ));
        }
        let slot: Arc<dyn SlotOps> = handle.slot.clone();
        if slot.begin(true)
            && let Err(error) = self.send(Job::Load {
                slot: slot.clone(),
                key: ty.key.into(),
                reload: true,
            })
        {
            slot.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }
    pub fn refresh(&self) -> Result<()> {
        self.send(Job::Refresh)
    }
    pub fn watch(&self, interval: Option<Duration>) {
        self.shared.watch_ms.store(
            interval.map_or(0, |d| (d.as_millis() as u64).max(50)),
            Ordering::Relaxed,
        );
    }
    pub fn drain_events(&self) -> Vec<AssetEvent> {
        self.shared.events.lock().drain(..).collect()
    }
    pub fn stats(&self) -> AssetStats {
        let c = &self.shared.counters;
        AssetStats {
            imports: c.imports.load(Ordering::Relaxed),
            decodes: c.decodes.load(Ordering::Relaxed),
            cache_hits: c.cache_hits.load(Ordering::Relaxed),
            source_bytes: c.source_bytes.load(Ordering::Relaxed),
            artifact_bytes: c.artifact_bytes.load(Ordering::Relaxed),
        }
    }
    fn registered<T: CookedAsset>(&self) -> Result<Arc<RegisteredType>> {
        self.shared
            .types
            .get(T::TYPE_KEY)
            .filter(|ty| ty.type_id == TypeId::of::<T>())
            .cloned()
            .ok_or_else(|| {
                AssetError::new(
                    ErrorKind::Registration,
                    format!("asset type not registered: {}", T::TYPE_KEY),
                )
            })
    }
    fn send(&self, job: Job) -> Result<()> {
        self.runtime
            .sender
            .lock()
            .as_ref()
            .ok_or_else(|| AssetError::new(ErrorKind::Shutdown, "asset server stopped"))?
            .try_send(job)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    AssetError::new(ErrorKind::QueueFull, "asset request queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    AssetError::new(ErrorKind::Shutdown, "asset worker stopped")
                }
            })
    }
}

pub struct LoadContext<'a> {
    work: &'a mut Work,
    dependencies: Vec<Dependency>,
}
impl LoadContext<'_> {
    pub fn dependency<T: CookedAsset>(&mut self, path: &AssetPath<T>) -> Result<Handle<T>> {
        let slot = self.work.resolve(path.address(), T::TYPE_KEY)?;
        self.dependencies.push(Dependency {
            address: path.address().clone(),
            type_key: T::TYPE_KEY.into(),
        });
        Handle::from_erased(slot)
    }
}

struct Bundle {
    manifest: Manifest,
    bytes: BTreeMap<String, Arc<Vec<u8>>>,
    imported: bool,
}
struct Staged {
    slot: Arc<dyn SlotOps>,
    value: ErasedValue,
    stamp: String,
    dependencies: Vec<Arc<dyn SlotOps>>,
}
struct Work {
    shared: Arc<Shared>,
    reload: bool,
    bundles: BTreeMap<AssetAddress, Bundle>,
    staged: BTreeMap<RequestKey, Staged>,
    order: Vec<RequestKey>,
    stack: Vec<RequestKey>,
    touched: Vec<Arc<dyn SlotOps>>,
}
impl Work {
    fn new(shared: Arc<Shared>, reload: bool) -> Self {
        Self {
            shared,
            reload,
            bundles: BTreeMap::new(),
            staged: BTreeMap::new(),
            order: Vec::new(),
            stack: Vec::new(),
            touched: Vec::new(),
        }
    }
    fn resolve(&mut self, address: &AssetAddress, expected: &str) -> Result<Arc<dyn SlotOps>> {
        let key = (address.clone(), expected.to_owned());
        if self.stack.contains(&key) {
            let chain = self
                .stack
                .iter()
                .map(|(p, _)| p.to_string())
                .chain(std::iter::once(address.to_string()))
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(AssetError::new(ErrorKind::DependencyCycle, chain));
        }
        if self.stack.len() >= 128 {
            return Err(AssetError::new(
                ErrorKind::DependencyCycle,
                "asset dependency depth exceeds 128",
            ));
        }
        if let Some(staged) = self.staged.get(&key) {
            return Ok(staged.slot.clone());
        }
        let ty = self.shared.types.get(expected).cloned().ok_or_else(|| {
            AssetError::new(
                ErrorKind::Registration,
                format!("unregistered asset type: {expected}"),
            )
        })?;
        if !self.reload
            && !self
                .bundles
                .get(&address.root())
                .is_some_and(|b| b.imported)
            && let Some(slot) = self.shared.find(address, &ty).filter(|slot| slot.ready())
        {
            return Ok(slot);
        }
        self.open_bundle(&address.root())?;
        let bundle = &self.bundles[&address.root()];
        let label = address.label().unwrap_or_default();
        let output = bundle.manifest.outputs.get(label).cloned().ok_or_else(|| {
            AssetError::new(
                ErrorKind::MissingAsset,
                format!("asset output not found: {address}"),
            )
        })?;
        if output.type_key != expected {
            return Err(AssetError::new(
                ErrorKind::TypeMismatch,
                format!("{address}: expected {expected}, found {}", output.type_key),
            ));
        }
        let bytes = bundle.bytes[label].clone();
        let imported = bundle.imported;
        let slot = self.shared.slot(address, &ty);
        self.touched.push(slot.clone());
        self.stack.push(key.clone());
        let mut context = LoadContext {
            work: self,
            dependencies: Vec::new(),
        };
        let result = ty.codec.decode(&bytes, &mut context);
        let mut dependencies = context.dependencies;
        dependencies.sort();
        dependencies.dedup();
        self.stack.pop();
        let value =
            result.map_err(|error| error.context(format!("loading {address} as {expected}")))?;
        if !imported && dependencies != output.dependencies {
            return Err(AssetError::new(
                ErrorKind::Cache,
                format!("dependency metadata disagrees with decoded asset: {address}"),
            ));
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(expected.as_bytes());
        hasher.update(&bytes);
        let mut handles = Vec::new();
        for dependency in &dependencies {
            let ty = &self.shared.types[&dependency.type_key];
            let handle = self.shared.slot(&dependency.address, ty);
            let dependency_key = (dependency.address.clone(), dependency.type_key.clone());
            let stamp = self
                .staged
                .get(&dependency_key)
                .map(|s| s.stamp.clone())
                .or_else(|| handle.stamp())
                .ok_or_else(|| {
                    AssetError::new(ErrorKind::InvalidData, "dependency was not prepared")
                })?;
            hasher.update(stamp.as_bytes());
            handles.push(handle);
        }
        self.bundles
            .get_mut(&address.root())
            .unwrap()
            .manifest
            .outputs
            .get_mut(label)
            .unwrap()
            .dependencies = dependencies;
        self.staged.insert(
            key.clone(),
            Staged {
                slot: slot.clone(),
                value,
                stamp: hasher.finalize().to_hex().to_string(),
                dependencies: handles,
            },
        );
        self.order.push(key);
        self.shared.counters.decodes.fetch_add(1, Ordering::Relaxed);
        Ok(slot)
    }
    fn open_bundle(&mut self, address: &AssetAddress) -> Result<()> {
        if self.bundles.contains_key(address) {
            return Ok(());
        }
        let cached = self
            .shared
            .cache
            .as_ref()
            .map(|cache| cache.read_manifest(address));
        if let Some(Ok(manifest)) = &cached {
            match self.read_cached(manifest.clone()) {
                Ok(bundle) => {
                    self.shared
                        .counters
                        .cache_hits
                        .fetch_add(1, Ordering::Relaxed);
                    self.bundles.insert(address.clone(), bundle);
                    return Ok(());
                }
                Err(error) if self.shared.packaged => {
                    return Err(error.context(format!("packaged asset {address}")));
                }
                Err(_) => {}
            }
        }
        if self.shared.packaged {
            return Err(cached
                .and_then(|r| r.err())
                .unwrap_or_else(|| AssetError::new(ErrorKind::Cache, "missing packaged cache"))
                .context(address.to_string()));
        }
        let selection = self.selection(address, cached.as_ref().and_then(|m| m.as_ref().ok()))?;
        let importer = self
            .shared
            .importers
            .get(&selection.importer)
            .cloned()
            .ok_or_else(|| {
                AssetError::new(
                    ErrorKind::UnsupportedImporter,
                    format!("importer not registered: {}", selection.importer),
                )
            })?;
        let mut ctx = ImportContext {
            address: address.clone(),
            types: &self.shared.types,
            sources: &self.shared.sources,
            outputs: BTreeMap::new(),
            inputs: BTreeMap::new(),
            source_bytes: 0,
        };
        let source_address =
            AssetAddress::parse(&format!("{}://{}", address.source(), address.path()))?;
        let bytes = ctx.read_source(&source_address)?;
        self.shared.counters.imports.fetch_add(1, Ordering::Relaxed);
        importer
            .import(&bytes, &selection.settings, &mut ctx)
            .map_err(|error| {
                error.context(format!("importing {address} with {}", importer.key()))
            })?;
        self.shared
            .counters
            .source_bytes
            .fetch_add(ctx.source_bytes, Ordering::Relaxed);
        if ctx.outputs.values().map(|o| o.bytes.len()).sum::<usize>() > MAX_ARTIFACT_BYTES {
            return Err(AssetError::new(
                ErrorKind::Import,
                "source bundle exceeds 512 MiB",
            ));
        }
        let mut outputs = BTreeMap::new();
        let mut payloads = BTreeMap::new();
        for (label, output) in ctx.outputs {
            outputs.insert(
                label.clone(),
                Output {
                    type_key: output.ty.key.into(),
                    schema: output.ty.schema,
                    codec: output.ty.codec.key().into(),
                    blob: String::new(),
                    length: output.bytes.len(),
                    dependencies: Vec::new(),
                },
            );
            payloads.insert(label, output.bytes);
        }
        self.bundles.insert(
            address.clone(),
            Bundle {
                manifest: Manifest {
                    format: 2,
                    source: address.clone(),
                    importer: importer.key().into(),
                    importer_version: importer.version(),
                    settings: serde_json::from_slice(&selection.settings)?,
                    target: self.shared.target.clone(),
                    inputs: ctx.inputs.into_iter().collect(),
                    outputs,
                },
                bytes: payloads,
                imported: true,
            },
        );
        Ok(())
    }
    fn selection(&self, address: &AssetAddress, manifest: Option<&Manifest>) -> Result<Selection> {
        if let Some(selection) = self.shared.selections.lock().get(address).cloned() {
            return Ok(selection);
        }
        if !address.variant().is_empty() {
            let manifest = manifest.ok_or_else(|| {
                AssetError::new(
                    ErrorKind::Cache,
                    format!(
                        "missing settings for variant {address}; load it with typed settings first"
                    ),
                )
            })?;
            return Ok(Selection {
                importer: manifest.importer.clone(),
                settings: settings_bytes(&manifest.settings)?,
            });
        }
        let extension = Path::new(address.path())
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let importer = self
            .shared
            .importers
            .values()
            .find(|i| i.extensions().contains(&extension.as_str()))
            .ok_or_else(|| {
                AssetError::new(
                    ErrorKind::UnsupportedImporter,
                    format!("no importer for {address}"),
                )
            })?;
        Ok(Selection {
            importer: importer.key().into(),
            settings: importer.settings()?,
        })
    }
    fn read_cached(&self, manifest: Manifest) -> Result<Bundle> {
        if manifest.target != self.shared.target {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "asset target profile changed",
            ));
        }
        if !self.shared.packaged {
            let selection = self.selection(&manifest.source, Some(&manifest))?;
            let importer = self
                .shared
                .importers
                .get(&selection.importer)
                .ok_or_else(|| {
                    AssetError::new(
                        ErrorKind::UnsupportedImporter,
                        "cached importer is not registered",
                    )
                })?;
            if manifest.importer != importer.key()
                || manifest.importer_version != importer.version()
                || settings_bytes(&manifest.settings)? != selection.settings
            {
                return Err(AssetError::new(
                    ErrorKind::Cache,
                    "importer or settings changed",
                ));
            }
            let root = AssetAddress::parse(&format!(
                "{}://{}",
                manifest.source.source(),
                manifest.source.path()
            ))?;
            if !manifest.inputs.iter().any(|(p, _)| p == &root) {
                return Err(AssetError::new(
                    ErrorKind::Cache,
                    "manifest has no root source fingerprint",
                ));
            }
            for (address, expected) in &manifest.inputs {
                let source = self.shared.sources.get(address.source()).ok_or_else(|| {
                    AssetError::new(ErrorKind::MissingAsset, "source namespace is not mounted")
                })?;
                let bytes = source.read(address.path())?;
                self.shared
                    .counters
                    .source_bytes
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                if blake3::hash(&bytes).to_hex().as_str() != expected {
                    return Err(AssetError::new(
                        ErrorKind::Cache,
                        format!("source changed: {address}"),
                    ));
                }
            }
        }
        if manifest
            .outputs
            .values()
            .map(|o| o.length as u64)
            .sum::<u64>()
            > MAX_ARTIFACT_BYTES as u64
        {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "source bundle exceeds size limit",
            ));
        }
        for output in manifest.outputs.values() {
            let ty = self.shared.types.get(&output.type_key).ok_or_else(|| {
                AssetError::new(
                    ErrorKind::Registration,
                    format!("unknown asset type: {}", output.type_key),
                )
            })?;
            if ty.schema != output.schema || ty.codec.key() != output.codec {
                return Err(AssetError::new(
                    ErrorKind::Cache,
                    "asset schema or codec changed",
                ));
            }
        }
        let bytes = read_payloads(&manifest, |output| {
            let payload = self.shared.cache.as_ref().unwrap().read_blob(output)?;
            self.shared
                .counters
                .artifact_bytes
                .fetch_add(payload.len() as u64, Ordering::Relaxed);
            Ok(payload)
        })?;
        Ok(Bundle {
            manifest,
            bytes,
            imported: false,
        })
    }
    fn finish(&mut self) -> Result<()> {
        loop {
            let next = self
                .bundles
                .iter()
                .filter(|(_, b)| b.imported)
                .flat_map(|(root, bundle)| {
                    bundle.manifest.outputs.iter().map(move |(label, output)| {
                        let address = if label.is_empty() {
                            root.clone()
                        } else {
                            root.with_label(label).expect("validated output label")
                        };
                        (address, output.type_key.clone())
                    })
                })
                .find(|key| !self.staged.contains_key(key));
            if let Some((address, key)) = next {
                self.resolve(&address, &key)?;
                continue;
            }
            let changed: HashSet<_> = self
                .staged
                .values()
                .filter(|s| s.slot.stamp().as_ref() != Some(&s.stamp))
                .map(|s| s.slot.id())
                .collect();
            let parent = self
                .shared
                .slots
                .lock()
                .iter()
                .find_map(|((address, type_id), slot)| {
                    let slot = slot.upgrade()?;
                    let ty = self
                        .shared
                        .types
                        .values()
                        .find(|ty| ty.type_id == *type_id)?;
                    let key = (address.clone(), ty.key.to_owned());
                    (!self.staged.contains_key(&key)
                        && slot.dependencies().iter().any(|id| changed.contains(id)))
                    .then_some(key)
                });
            let Some((address, key)) = parent else {
                break;
            };
            self.reload = true;
            self.resolve(&address, &key)?;
        }
        if let Some(cache) = &self.shared.cache {
            for bundle in self.bundles.values_mut().filter(|b| b.imported) {
                for (label, output) in &mut bundle.manifest.outputs {
                    cache.write_blob(output, &bundle.bytes[label])?;
                }
            }
            for bundle in self.bundles.values().filter(|b| b.imported) {
                cache.write_manifest(&bundle.manifest)?;
            }
        }
        for (address, bundle) in &self.bundles {
            self.shared
                .builds
                .lock()
                .insert(address.clone(), bundle.manifest.clone());
        }
        let mut retired = Vec::new();
        let _publication = self.shared.publication.write();
        let removed: Vec<_> = self
            .shared
            .slots
            .lock()
            .values()
            .filter_map(Weak::upgrade)
            .filter(|slot| {
                let Some(address) = slot.address() else {
                    return false;
                };
                self.bundles.get(&address.root()).is_some_and(|bundle| {
                    bundle.imported
                        && !bundle
                            .manifest
                            .outputs
                            .contains_key(address.label().unwrap_or_default())
                })
            })
            .collect();
        for slot in removed {
            let address = slot.address().unwrap().clone();
            let error = AssetError::new(
                ErrorKind::MissingAsset,
                format!("output removed during reload: {address}"),
            );
            slot.fail(error.clone());
            self.shared.event(AssetEvent::Failed {
                id: slot.id(),
                address,
                error,
            });
        }
        for key in &self.order {
            let staged = &self.staged[key];
            let (revision, changed, previous) = staged.slot.publish(
                staged.value.clone(),
                staged.stamp.clone(),
                staged.dependencies.clone(),
            )?;
            retired.extend(previous);
            if changed {
                self.shared.event(AssetEvent::Ready {
                    id: staged.slot.id(),
                    address: key.0.clone(),
                    revision,
                });
            }
        }
        Ok(())
    }
}
fn read_payloads(
    manifest: &Manifest,
    read: impl Fn(&Output) -> Result<Vec<u8>> + Sync,
) -> Result<BTreeMap<String, Arc<Vec<u8>>>> {
    let load = |(label, output): (&String, &Output)| {
        let payload = read(output).map_err(|error| {
            error.context(format!("reading {} output {label:?}", manifest.source))
        })?;
        Ok((label.clone(), Arc::new(payload)))
    };
    #[cfg(feature = "parallel")]
    let bytes = manifest.outputs.par_iter().map(load).collect();
    #[cfg(not(feature = "parallel"))]
    let bytes = manifest.outputs.iter().map(load).collect();
    bytes
}

fn process(shared: &Arc<Shared>, slot: Arc<dyn SlotOps>, key: String, reload: bool) {
    if !reload && slot.ready() {
        slot.finish();
        return;
    }
    let Some(address) = slot.address().cloned() else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lock = if !shared.packaged {
            shared.cache.as_ref().map(Cache::lock).transpose()?
        } else {
            None
        };
        let mut work = Work::new(shared.clone(), reload);
        let result = work.resolve(&address, &key).and_then(|_| work.finish());
        if let Err(error) = &result {
            for touched in &work.touched {
                if !touched.ready() {
                    touched.fail(error.clone());
                }
            }
        }
        result
    }))
    .unwrap_or_else(|_| {
        Err(AssetError::new(
            ErrorKind::Import,
            "asset extension panicked",
        ))
    });
    if let Err(error) = result {
        slot.fail(error.clone());
        shared.event(AssetEvent::Failed {
            id: slot.id(),
            address,
            error,
        });
    }
}

#[cfg(test)]
mod tests;
fn refresh(shared: &Arc<Shared>) {
    if shared.packaged {
        return;
    }
    let slots: Vec<_> = shared
        .slots
        .lock()
        .iter()
        .filter_map(|((_, type_id), weak)| {
            let slot = weak.upgrade()?;
            let key = shared
                .types
                .values()
                .find(|ty| ty.type_id == *type_id)?
                .key
                .to_owned();
            Some((slot, key))
        })
        .collect();
    let mut roots = BTreeMap::new();
    for (slot, key) in slots {
        let Some(address) = slot.address() else {
            continue;
        };
        roots
            .entry(address.root())
            .or_insert_with(|| (slot.clone(), key));
        if address.label().is_none() {
            let key = shared
                .types
                .values()
                .find(|ty| {
                    shared
                        .find(address, ty)
                        .is_some_and(|found| found.id() == slot.id())
                })
                .map(|ty| ty.key.to_owned());
            if let Some(key) = key {
                roots.insert(address.root(), (slot, key));
            }
        }
    }
    shared
        .builds
        .lock()
        .retain(|address, _| roots.contains_key(address));
    let mut fingerprints = BTreeMap::new();
    for (address, (slot, key)) in roots {
        let manifest = shared.builds.lock().get(&address).cloned();
        let changed = manifest.is_none_or(|manifest| {
            manifest.inputs.iter().any(|(address, expected)| {
                let fingerprint = fingerprints.entry(address.clone()).or_insert_with(|| {
                    let source = shared.sources.get(address.source())?;
                    let bytes = source.read(address.path()).ok()?;
                    shared
                        .counters
                        .source_bytes
                        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    Some(blake3::hash(&bytes).to_hex().to_string())
                });
                fingerprint.as_ref() != Some(expected)
            })
        });
        if (changed || slot.stamp().is_none() || slot.has_error()) && slot.begin(true) {
            process(shared, slot, key, true);
        }
    }
}
