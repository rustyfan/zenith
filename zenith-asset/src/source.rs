use crate::{AssetAddress, AssetError, ErrorKind, Result};
use parking_lot::RwLock;
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
pub const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

pub trait AssetSource: Send + Sync + 'static {
    fn read(&self, path: &str) -> Result<Vec<u8>>;
}
impl<S: AssetSource> AssetSource for Arc<S> {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        (**self).read(path)
    }
}
pub struct FileSource {
    root: PathBuf,
}
impl FileSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}
impl AssetSource for FileSource {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let logical = AssetAddress::parse(path)?;
        if logical.source() != "default" || logical.label().is_some() {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                "file sources require a relative file path",
            ));
        }
        let root = self.root.canonicalize()?;
        let full = root
            .join(logical.path())
            .canonicalize()
            .map_err(|e| AssetError::from(e).context(path))?;
        if !full.starts_with(&root) {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                format!("source escapes {}: {path}", root.display()),
            ));
        }
        let file = File::open(full)?;
        if file.metadata()?.len() > MAX_SOURCE_BYTES {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "source exceeds 512 MiB",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(AssetError::new(
                ErrorKind::InvalidData,
                "source grew beyond 512 MiB",
            ));
        }
        Ok(bytes)
    }
}
#[derive(Default)]
pub struct MemorySource {
    files: RwLock<BTreeMap<String, Vec<u8>>>,
}
impl MemorySource {
    pub fn insert(&self, path: &str, bytes: impl Into<Vec<u8>>) -> Result<()> {
        let address = AssetAddress::parse(path)?;
        if address.source() != "default" || address.label().is_some() {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                "memory sources require a relative file path",
            ));
        }
        self.files
            .write()
            .insert(address.path().to_owned(), bytes.into());
        Ok(())
    }
    pub fn remove(&self, path: &str) -> Result<bool> {
        Ok(self
            .files
            .write()
            .remove(AssetAddress::parse(path)?.path())
            .is_some())
    }
}
impl AssetSource for MemorySource {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.files.read().get(path).cloned().ok_or_else(|| {
            AssetError::new(ErrorKind::MissingAsset, format!("source not found: {path}"))
        })
    }
}
