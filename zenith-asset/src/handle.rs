use crate::{Asset, AssetAddress, AssetError, ErasedValue, ErrorKind, Result};
use parking_lot::{Condvar, Mutex, RwLock};
use std::{any::Any, fmt, ops::Deref, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId {
    pub server: u64,
    pub slot: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision(pub u64);
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CpuRetention {
    #[default]
    Keep,
    ReleaseAfterUpload,
}
#[derive(Debug, Clone)]
pub enum LoadState {
    Loading,
    Reloading { revision: Revision },
    Ready { revision: Revision },
    CpuReleased { revision: Revision },
    Failed(AssetError),
}

pub struct AssetSnapshot<T> {
    pub revision: Revision,
    pub value: Arc<T>,
}
impl<T> Clone for AssetSnapshot<T> {
    fn clone(&self) -> Self {
        Self {
            revision: self.revision,
            value: self.value.clone(),
        }
    }
}
impl<T> Deref for AssetSnapshot<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

pub struct Handle<T: Asset> {
    pub(crate) slot: Arc<Slot<T>>,
}
impl<T: Asset> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
        }
    }
}
impl<T: Asset> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("id", &self.id())
            .field("address", &self.address())
            .finish()
    }
}
impl<T: Asset> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}
impl<T: Asset> Eq for Handle<T> {}
impl<T: Asset> std::hash::Hash for Handle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}
impl<T: Asset> Handle<T> {
    pub fn id(&self) -> AssetId {
        self.slot.id
    }
    pub fn address(&self) -> Option<&AssetAddress> {
        self.slot.address.as_ref()
    }
    pub fn path(&self) -> Option<crate::AssetPath<T>> {
        self.address().cloned().map(crate::AssetPath::from_address)
    }
    pub fn get(&self) -> Option<Arc<T>> {
        self.snapshot().map(|snapshot| snapshot.value)
    }
    pub fn snapshot(&self) -> Option<AssetSnapshot<T>> {
        self.slot.state.lock().value.clone()
    }
    pub fn revision(&self) -> Option<Revision> {
        self.slot.state.lock().revision
    }
    pub fn cpu_retention(&self) -> CpuRetention {
        self.slot.retention
    }
    pub fn release_cpu(&self, snapshot: &AssetSnapshot<T>) -> bool {
        if self.slot.retention == CpuRetention::Keep {
            return false;
        }
        let retired = {
            let _publication = self.slot.publication.write();
            let mut state = self.slot.state.lock();
            if state.value.as_ref().is_some_and(|current| {
                current.revision == snapshot.revision
                    && Arc::ptr_eq(&current.value, &snapshot.value)
            }) {
                state.value.take()
            } else {
                None
            }
        };
        retired.is_some()
    }
    pub fn with_snapshot<R>(&self, read: impl FnOnce(Option<AssetSnapshot<T>>) -> R) -> R {
        let _guard = self.slot.publication.read();
        read(self.snapshot())
    }
    pub fn last_error(&self) -> Option<AssetError> {
        self.slot.state.lock().error.clone()
    }
    pub fn state(&self) -> LoadState {
        let state = self.slot.state.lock();
        match (state.revision, state.running, &state.error) {
            (Some(revision), true, _) => LoadState::Reloading { revision },
            (Some(revision), false, _) if state.value.is_some() => LoadState::Ready { revision },
            (Some(revision), false, _) => LoadState::CpuReleased { revision },
            (None, false, Some(error)) => LoadState::Failed(error.clone()),
            _ => LoadState::Loading,
        }
    }
    pub fn wait(&self) -> Result<Arc<T>> {
        let mut state = self.slot.state.lock();
        while state.running {
            self.slot.changed.wait(&mut state);
        }
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        state
            .value
            .as_ref()
            .map(|s| s.value.clone())
            .ok_or_else(|| {
                if state.revision.is_some() {
                    AssetError::new(
                        ErrorKind::CpuReleased,
                        "CPU asset data was released; load or reload it before reading",
                    )
                } else {
                    AssetError::new(ErrorKind::InvalidData, "asset has not been published")
                }
            })
    }
    pub(crate) fn from_erased(slot: Arc<dyn SlotOps>) -> Result<Self> {
        let any: Arc<dyn Any + Send + Sync> = slot;
        Ok(Self {
            slot: any.downcast::<Slot<T>>().map_err(|_| {
                AssetError::new(ErrorKind::TypeMismatch, "asset handle type mismatch")
            })?,
        })
    }
}
struct State<T> {
    value: Option<AssetSnapshot<T>>,
    revision: Option<Revision>,
    stamp: Option<String>,
    running: bool,
    error: Option<AssetError>,
    dependencies: Vec<Arc<dyn SlotOps>>,
}
pub(crate) struct Slot<T> {
    id: AssetId,
    address: Option<AssetAddress>,
    state: Mutex<State<T>>,
    changed: Condvar,
    publication: Arc<RwLock<()>>,
    retention: CpuRetention,
}
impl<T: Asset> Slot<T> {
    pub(crate) fn new(
        id: AssetId,
        address: Option<AssetAddress>,
        publication: Arc<RwLock<()>>,
        retention: CpuRetention,
    ) -> Self {
        Self {
            id,
            address,
            state: Mutex::new(State {
                value: None,
                revision: None,
                stamp: None,
                running: false,
                error: None,
                dependencies: Vec::new(),
            }),
            changed: Condvar::new(),
            publication,
            retention,
        }
    }
}
type Publication = (Revision, bool, Vec<ErasedValue>);

pub(crate) trait SlotOps: Any + Send + Sync {
    fn id(&self) -> AssetId;
    fn address(&self) -> Option<&AssetAddress>;
    fn ready(&self) -> bool;
    fn has_error(&self) -> bool;
    fn stamp(&self) -> Option<String>;
    fn dependencies(&self) -> Vec<AssetId>;
    fn begin(&self, reload: bool) -> bool;
    fn publish(
        &self,
        value: ErasedValue,
        stamp: String,
        dependencies: Vec<Arc<dyn SlotOps>>,
    ) -> Result<Publication>;
    fn fail(&self, error: AssetError);
    fn finish(&self);
}
impl<T: Asset> SlotOps for Slot<T> {
    fn id(&self) -> AssetId {
        self.id
    }
    fn address(&self) -> Option<&AssetAddress> {
        self.address.as_ref()
    }
    fn ready(&self) -> bool {
        self.state.lock().value.is_some()
    }
    fn has_error(&self) -> bool {
        self.state.lock().error.is_some()
    }
    fn stamp(&self) -> Option<String> {
        self.state.lock().stamp.clone()
    }
    fn dependencies(&self) -> Vec<AssetId> {
        self.state
            .lock()
            .dependencies
            .iter()
            .map(|d| d.id())
            .collect()
    }
    fn begin(&self, reload: bool) -> bool {
        let mut state = self.state.lock();
        if state.running || (!reload && state.value.is_some()) {
            return false;
        }
        state.running = true;
        state.error = None;
        true
    }
    fn publish(
        &self,
        value: ErasedValue,
        stamp: String,
        dependencies: Vec<Arc<dyn SlotOps>>,
    ) -> Result<Publication> {
        let value = value.downcast::<T>().map_err(|_| {
            AssetError::new(ErrorKind::TypeMismatch, "asset publication type mismatch")
        })?;
        let mut state = self.state.lock();
        let changed = state.stamp.as_ref() != Some(&stamp);
        let revision = Revision(state.revision.map_or(0, |r| r.0) + u64::from(changed));
        let mut retired: Vec<ErasedValue> = Vec::new();
        if changed || state.value.is_none() {
            if let Some(previous) = state.value.replace(AssetSnapshot { revision, value }) {
                retired.push(previous.value);
            }
            state.stamp = Some(stamp);
            state.revision = Some(revision);
            for dependency in std::mem::replace(&mut state.dependencies, dependencies) {
                retired.push(dependency);
            }
        }
        state.running = false;
        state.error = None;
        self.changed.notify_all();
        Ok((revision, changed, retired))
    }
    fn fail(&self, error: AssetError) {
        let mut state = self.state.lock();
        state.running = false;
        state.error = Some(error);
        self.changed.notify_all();
    }
    fn finish(&self) {
        self.state.lock().running = false;
        self.changed.notify_all();
    }
}
