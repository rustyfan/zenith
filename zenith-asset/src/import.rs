use crate::server::RegisteredType;
use crate::{
    AssetAddress, AssetError, AssetPath, AssetSource, CookedAsset, ErasedValue, ErrorKind,
    LoadContext, Result,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    marker::PhantomData,
    sync::Arc,
};

pub const MAX_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;

pub trait AssetCodec<T: CookedAsset>: Send + Sync + 'static {
    fn key(&self) -> &'static str;
    fn encode(&self, data: &T::Data) -> Result<Vec<u8>>;
    fn decode(&self, bytes: &[u8]) -> Result<T::Data>;
}
pub struct SerdeCodec;
impl<T: CookedAsset> AssetCodec<T> for SerdeCodec {
    fn key(&self) -> &'static str {
        "bincode-serde-2"
    }
    fn encode(&self, data: &T::Data) -> Result<Vec<u8>> {
        Self::encode_data(data)
    }
    fn decode(&self, bytes: &[u8]) -> Result<T::Data> {
        Self::decode_data(bytes)
    }
}
impl SerdeCodec {
    pub(crate) fn encode_data<T: Serialize>(data: &T) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(
            data,
            bincode::config::standard().with_limit::<MAX_ARTIFACT_BYTES>(),
        )
        .map_err(|e| AssetError::caused_by(ErrorKind::InvalidData, "encode asset", e))
    }
    pub(crate) fn decode_data<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
        let (data, read) = bincode::serde::decode_from_slice(
            bytes,
            bincode::config::standard().with_limit::<MAX_ARTIFACT_BYTES>(),
        )
        .map_err(|e| AssetError::caused_by(ErrorKind::InvalidData, "decode asset", e))?;
        if read != bytes.len() {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "trailing bytes in asset payload",
            ));
        }
        Ok(data)
    }
}
pub(crate) trait ErasedCodec: Send + Sync {
    fn key(&self) -> &'static str;
    fn encode(&self, data: &dyn Any) -> Result<Vec<u8>>;
    fn decode(&self, bytes: &[u8], ctx: &mut LoadContext<'_>) -> Result<ErasedValue>;
}
pub(crate) struct CodecAdapter<T, C>(pub C, pub PhantomData<fn(T) -> T>);
impl<T: CookedAsset, C: AssetCodec<T>> ErasedCodec for CodecAdapter<T, C> {
    fn key(&self) -> &'static str {
        self.0.key()
    }
    fn encode(&self, data: &dyn Any) -> Result<Vec<u8>> {
        self.0.encode(
            data.downcast_ref::<T::Data>().ok_or_else(|| {
                AssetError::new(ErrorKind::TypeMismatch, "cooked data type mismatch")
            })?,
        )
    }
    fn decode(&self, bytes: &[u8], ctx: &mut LoadContext<'_>) -> Result<ErasedValue> {
        Ok(Arc::new(T::from_data(self.0.decode(bytes)?, ctx)?))
    }
}

pub trait Importer: Send + Sync + 'static {
    type Settings: Default + Serialize + DeserializeOwned + Send + Sync + 'static;
    type Output: CookedAsset;
    const KEY: &'static str;
    const VERSION: u32;
    fn extensions(&self) -> &[&str];
    fn import(
        &self,
        bytes: &[u8],
        settings: &Self::Settings,
        ctx: &mut ImportContext<'_>,
    ) -> Result<<Self::Output as CookedAsset>::Data>;
}
pub(crate) trait ErasedImporter: Send + Sync {
    fn key(&self) -> &'static str;
    fn version(&self) -> u32;
    fn output(&self) -> &'static str;
    fn extensions(&self) -> &[&str];
    fn settings(&self) -> Result<Vec<u8>>;
    fn import(&self, bytes: &[u8], settings: &[u8], ctx: &mut ImportContext<'_>) -> Result<()>;
}
pub(crate) struct ImporterAdapter<I>(pub I);
impl<I: Importer> ErasedImporter for ImporterAdapter<I> {
    fn key(&self) -> &'static str {
        I::KEY
    }
    fn version(&self) -> u32 {
        I::VERSION
    }
    fn output(&self) -> &'static str {
        I::Output::TYPE_KEY
    }
    fn extensions(&self) -> &[&str] {
        self.0.extensions()
    }
    fn settings(&self) -> Result<Vec<u8>> {
        settings_bytes(&I::Settings::default())
    }
    fn import(&self, bytes: &[u8], settings: &[u8], ctx: &mut ImportContext<'_>) -> Result<()> {
        let settings: I::Settings = serde_json::from_slice(settings)?;
        let root = self.0.import(bytes, &settings, ctx)?;
        ctx.emit_root::<I::Output>(root)
    }
}
pub(crate) fn settings_bytes(settings: &impl Serialize) -> Result<Vec<u8>> {
    fn sort(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for value in map.values_mut() {
                    sort(value);
                }
                map.sort_keys();
            }
            serde_json::Value::Array(values) => values.iter_mut().for_each(sort),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(settings)?;
    sort(&mut value);
    Ok(serde_json::to_vec(&value)?)
}
pub(crate) struct ImportedOutput {
    pub ty: Arc<RegisteredType>,
    pub bytes: Arc<Vec<u8>>,
}
pub struct ImportContext<'a> {
    pub(crate) address: AssetAddress,
    pub(crate) types: &'a BTreeMap<String, Arc<RegisteredType>>,
    pub(crate) sources: &'a BTreeMap<String, Arc<dyn AssetSource>>,
    pub(crate) outputs: BTreeMap<String, ImportedOutput>,
    pub(crate) inputs: BTreeMap<AssetAddress, String>,
    pub(crate) source_bytes: u64,
}
impl ImportContext<'_> {
    pub fn address(&self) -> &AssetAddress {
        &self.address
    }
    pub fn path<T>(&self, label: &str) -> Result<AssetPath<T>> {
        Ok(AssetPath::from_address(self.address.with_label(label)?))
    }
    pub fn read_relative(&mut self, path: &str) -> Result<Vec<u8>> {
        let address = self.address.relative(path)?;
        self.read_source(&address)
    }
    pub fn read_source(&mut self, address: &AssetAddress) -> Result<Vec<u8>> {
        if address.label().is_some() || !address.variant().is_empty() {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                "source reads cannot reference a label or variant",
            ));
        }
        let source = self.sources.get(address.source()).ok_or_else(|| {
            AssetError::new(
                ErrorKind::MissingAsset,
                format!("source namespace not mounted: {}", address.source()),
            )
        })?;
        let bytes = source.read(address.path())?;
        if bytes.len() as u64 > crate::source::MAX_SOURCE_BYTES {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "source exceeds size limit",
            ));
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        if let Some(previous) = self.inputs.insert(address.clone(), hash.clone())
            && previous != hash
        {
            return Err(AssetError::new(
                ErrorKind::Import,
                format!("source changed during import: {address}"),
            ));
        }
        self.source_bytes += bytes.len() as u64;
        Ok(bytes)
    }
    pub fn emit<T: CookedAsset>(&mut self, label: &str, data: T::Data) -> Result<AssetPath<T>> {
        let path = self.path(label)?;
        self.insert::<T>(path.address().label().unwrap_or_default(), data)?;
        Ok(path)
    }
    fn emit_root<T: CookedAsset>(&mut self, data: T::Data) -> Result<()> {
        self.insert::<T>("", data)
    }
    fn insert<T: CookedAsset>(&mut self, label: &str, data: T::Data) -> Result<()> {
        let ty = self
            .types
            .get(T::TYPE_KEY)
            .filter(|t| t.type_id == TypeId::of::<T>())
            .ok_or_else(|| {
                AssetError::new(
                    ErrorKind::Registration,
                    format!("asset type is not registered: {}", T::TYPE_KEY),
                )
            })?
            .clone();
        if self.outputs.contains_key(label) {
            return Err(AssetError::new(
                ErrorKind::Import,
                format!("duplicate output label: {label}"),
            ));
        }
        if self.outputs.len() >= 65536 {
            return Err(AssetError::new(
                ErrorKind::Import,
                "too many importer outputs",
            ));
        }
        let bytes = ty.codec.encode(&data)?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "cooked asset exceeds size limit",
            ));
        }
        self.outputs.insert(
            label.to_owned(),
            ImportedOutput {
                ty,
                bytes: Arc::new(bytes),
            },
        );
        Ok(())
    }
}
