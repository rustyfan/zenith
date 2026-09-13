use crate::import::MAX_ARTIFACT_BYTES;
use crate::{AssetAddress, AssetError, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAGIC: &[u8; 8] = b"ZENAS002";
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Dependency {
    pub address: AssetAddress,
    pub type_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Output {
    pub type_key: String,
    pub schema: u32,
    pub codec: String,
    pub blob: String,
    pub length: usize,
    pub dependencies: Vec<Dependency>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub format: u32,
    pub source: AssetAddress,
    pub importer: String,
    pub importer_version: u32,
    pub settings: serde_json::Value,
    pub target: String,
    pub inputs: Vec<(AssetAddress, String)>,
    pub outputs: BTreeMap<String, Output>,
}
#[derive(Serialize, Deserialize)]
struct Header {
    type_key: String,
    schema: u32,
    codec: String,
    compression: u8,
    length: usize,
}
pub(crate) struct Cache {
    root: PathBuf,
    target: String,
}
impl Cache {
    pub(crate) fn new(root: PathBuf, target: String) -> Self {
        Self { root, target }
    }
    pub(crate) fn lock(&self) -> Result<File> {
        fs::create_dir_all(self.root.join("manifests"))?;
        fs::create_dir_all(self.root.join("blobs"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("writer.lock"))?;
        lock.lock()?;
        Ok(lock)
    }
    fn manifest_path(&self, address: &AssetAddress) -> Result<PathBuf> {
        let digest = blake3::hash(&serde_json::to_vec(&(address.root(), &self.target))?).to_hex();
        Ok(self.root.join("manifests").join(format!("{digest}.json")))
    }
    pub(crate) fn read_manifest(&self, address: &AssetAddress) -> Result<Manifest> {
        let bytes = limited_read(&self.manifest_path(address)?, MAX_MANIFEST_BYTES)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        if manifest.format != 2
            || manifest.source != address.root()
            || manifest.outputs.is_empty()
            || manifest.outputs.len() > 65536
        {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "incompatible asset manifest",
            ));
        }
        for (label, output) in &manifest.outputs {
            if !label.is_empty() {
                address.with_label(label)?;
            }
            if output.length > MAX_ARTIFACT_BYTES
                || output.dependencies.len() > 65536
                || !digest_valid(&output.blob)
            {
                return Err(AssetError::new(
                    ErrorKind::Cache,
                    "invalid artifact metadata",
                ));
            }
        }
        if manifest
            .inputs
            .iter()
            .any(|(address, digest)| address.label().is_some() || !digest_valid(digest))
        {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "invalid source input metadata",
            ));
        }
        Ok(manifest)
    }
    pub(crate) fn read_blob(&self, output: &Output) -> Result<Vec<u8>> {
        if !digest_valid(&output.blob) || output.length > MAX_ARTIFACT_BYTES {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "invalid artifact size or digest",
            ));
        }
        let path = self.root.join("blobs").join(format!("{}.bin", output.blob));
        let mut file = File::open(&path)?;
        if file.metadata()?.len() > MAX_ARTIFACT_BYTES as u64 + 1024 * 1024 {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "artifact file exceeds size limit",
            ));
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(&mut file)?;
        if hasher.finalize().to_hex().as_str() != output.blob {
            return Err(AssetError::new(
                ErrorKind::Cache,
                format!("artifact integrity failure: {}", output.blob),
            ));
        }
        file.rewind()?;
        let mut reader = BufReader::new(file);
        let mut magic = [0; 8];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "unknown artifact container",
            ));
        }
        let mut size = [0; 4];
        reader.read_exact(&mut size)?;
        let size = u32::from_le_bytes(size) as usize;
        if size > 16384 {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "artifact header too large",
            ));
        }
        let mut header = vec![0; size];
        reader.read_exact(&mut header)?;
        let header: Header = serde_json::from_slice(&header)?;
        if header.type_key != output.type_key
            || header.schema != output.schema
            || header.codec != output.codec
            || header.length != output.length
        {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "artifact header disagrees with manifest",
            ));
        }
        let mut bytes = Vec::new();
        match header.compression {
            0 => {
                reader
                    .take(output.length as u64 + 1)
                    .read_to_end(&mut bytes)?;
            }
            1 => {
                zstd::stream::read::Decoder::new(reader)?
                    .take(output.length as u64 + 1)
                    .read_to_end(&mut bytes)?;
            }
            _ => {
                return Err(AssetError::new(
                    ErrorKind::Cache,
                    "unknown artifact compression",
                ));
            }
        }
        if bytes.len() != output.length {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "artifact decoded length mismatch",
            ));
        }
        Ok(bytes)
    }
    pub(crate) fn write_blob(&self, output: &mut Output, bytes: &[u8]) -> Result<()> {
        if bytes.len() != output.length || bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "invalid artifact payload length",
            ));
        }
        let temp = Temporary::new(&self.root.join("blobs"))?;
        let mut writer = BufWriter::new(OpenOptions::new().write(true).open(&temp.path)?);
        let header = serde_json::to_vec(&Header {
            type_key: output.type_key.clone(),
            schema: output.schema,
            codec: output.codec.clone(),
            compression: 1,
            length: bytes.len(),
        })?;
        writer.write_all(MAGIC)?;
        writer.write_all(&(header.len() as u32).to_le_bytes())?;
        writer.write_all(&header)?;
        let mut encoder = zstd::stream::write::Encoder::new(writer, 3)?;
        encoder.write_all(bytes)?;
        let mut writer = encoder.finish()?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(File::open(&temp.path)?)?;
        output.blob = hasher.finalize().to_hex().to_string();
        fs::rename(
            &temp.path,
            self.root.join("blobs").join(format!("{}.bin", output.blob)),
        )?;
        #[cfg(unix)]
        File::open(self.root.join("blobs"))?.sync_all()?;
        Ok(())
    }
    pub(crate) fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(manifest)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(AssetError::new(
                ErrorKind::Cache,
                "manifest exceeds size limit",
            ));
        }
        let temp = Temporary::new(&self.root.join("manifests"))?;
        let mut file = OpenOptions::new().write(true).open(&temp.path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp.path, self.manifest_path(&manifest.source)?)?;
        #[cfg(unix)]
        File::open(self.root.join("manifests"))?.sync_all()?;
        Ok(())
    }
}
fn digest_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn limited_read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(AssetError::new(
            ErrorKind::Cache,
            "metadata exceeds size limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(AssetError::new(
            ErrorKind::Cache,
            "metadata exceeds size limit",
        ));
    }
    Ok(bytes)
}
struct Temporary {
    path: PathBuf,
}
impl Temporary {
    fn new(parent: &Path) -> Result<Self> {
        loop {
            let path = parent.join(format!(
                ".tmp-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            match File::create_new(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
