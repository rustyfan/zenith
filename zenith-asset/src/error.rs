use std::{error::Error, fmt, sync::Arc};
pub type Result<T> = std::result::Result<T, AssetError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidPath,
    Registration,
    UnsupportedImporter,
    TypeMismatch,
    MissingAsset,
    CpuReleased,
    DependencyCycle,
    InvalidData,
    Cache,
    Io,
    QueueFull,
    Shutdown,
    Import,
}

#[derive(Debug, Clone)]
pub struct AssetError {
    pub kind: ErrorKind,
    message: String,
    cause: Option<Arc<dyn Error + Send + Sync>>,
}

impl AssetError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            cause: None,
        }
    }
    pub fn context(self, message: impl Into<String>) -> Self {
        Self {
            kind: self.kind,
            message: message.into(),
            cause: Some(Arc::new(self)),
        }
    }
    pub fn caused_by(
        kind: ErrorKind,
        message: impl Into<String>,
        cause: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            cause: Some(Arc::new(cause)),
        }
    }
}
impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}
impl Error for AssetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_ref().map(|cause| cause.as_ref() as _)
    }
}
impl From<std::io::Error> for AssetError {
    fn from(error: std::io::Error) -> Self {
        Self::caused_by(ErrorKind::Io, "asset I/O", error)
    }
}
impl From<serde_json::Error> for AssetError {
    fn from(error: serde_json::Error) -> Self {
        Self::caused_by(ErrorKind::InvalidData, "asset metadata", error)
    }
}
