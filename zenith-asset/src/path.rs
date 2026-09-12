use crate::{AssetError, ErrorKind, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    str::FromStr,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct AssetAddress {
    source: String,
    path: String,
    label: Option<String>,
    variant: String,
}
impl AssetAddress {
    pub fn parse(value: &str) -> Result<Self> {
        let (source, rest) = value.split_once("://").unwrap_or(("default", value));
        let (path, label) = rest
            .split_once('#')
            .map_or((rest, None), |(p, l)| (p, Some(l)));
        if source.is_empty()
            || !source
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                "invalid asset source namespace",
            ));
        }
        Ok(Self {
            source: source.to_owned(),
            path: normalize(path)?,
            label: label.map(normalize).transpose()?,
            variant: String::new(),
        })
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    pub fn variant(&self) -> &str {
        &self.variant
    }
    pub fn with_label(&self, label: &str) -> Result<Self> {
        let mut address = self.clone();
        address.label = Some(normalize(label)?);
        Ok(address)
    }
    pub(crate) fn root(&self) -> Self {
        let mut address = self.clone();
        address.label = None;
        address
    }
    pub(crate) fn set_variant(&mut self, variant: String) {
        self.variant = variant;
    }
    pub(crate) fn relative(&self, path: &str) -> Result<Self> {
        let path = path.replace('\\', "/");
        if path.starts_with('/') || path.contains([':', '#', '?', '\0']) {
            return Err(AssetError::new(
                ErrorKind::InvalidPath,
                format!("invalid relative source path: {path}"),
            ));
        }
        let mut parts: Vec<_> = self.path.split('/').collect();
        parts.pop();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return Err(AssetError::new(
                            ErrorKind::InvalidPath,
                            "source path escapes its root",
                        ));
                    }
                }
                part => parts.push(part),
            }
        }
        Self::parse(&format!("{}://{}", self.source, parts.join("/")))
    }
}
fn normalize(path: &str) -> Result<String> {
    let path = path.replace('\\', "/");
    if path.is_empty() || path.starts_with('/') || path.contains([':', '#', '?', '\0']) {
        return Err(AssetError::new(
            ErrorKind::InvalidPath,
            format!("invalid asset path: {path}"),
        ));
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(AssetError::new(
                    ErrorKind::InvalidPath,
                    "parent components are not allowed in an asset address",
                ));
            }
            part if part.ends_with(['.', ' ']) => {
                return Err(AssetError::new(
                    ErrorKind::InvalidPath,
                    "asset path has a platform-ambiguous component",
                ));
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(AssetError::new(ErrorKind::InvalidPath, "empty asset path"));
    }
    Ok(parts.join("/"))
}
impl<'de> Deserialize<'de> for AssetAddress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Data {
            source: String,
            path: String,
            label: Option<String>,
            variant: String,
        }
        let data = Data::deserialize(deserializer)?;
        let mut address = Self::parse(&format!("{}://{}", data.source, data.path))
            .map_err(serde::de::Error::custom)?;
        if let Some(label) = data.label {
            address = address
                .with_label(&label)
                .map_err(serde::de::Error::custom)?;
        }
        if !data.variant.is_empty()
            && (data.variant.len() != 64 || !data.variant.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(serde::de::Error::custom("invalid asset variant digest"));
        }
        address.variant = data.variant;
        Ok(address)
    }
}
impl fmt::Display for AssetAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.source, self.path)?;
        if let Some(label) = &self.label {
            write!(f, "#{label}")?;
        }
        if !self.variant.is_empty() {
            write!(f, " [{}]", self.variant)?;
        }
        Ok(())
    }
}
pub struct AssetPath<T> {
    address: AssetAddress,
    marker: PhantomData<fn(T) -> T>,
}
impl<T> AssetPath<T> {
    pub fn new(value: &str) -> Result<Self> {
        Ok(Self::from_address(AssetAddress::parse(value)?))
    }
    pub fn address(&self) -> &AssetAddress {
        &self.address
    }
    pub fn from_address(address: AssetAddress) -> Self {
        Self {
            address,
            marker: PhantomData,
        }
    }
}
impl<T> Clone for AssetPath<T> {
    fn clone(&self) -> Self {
        Self::from_address(self.address.clone())
    }
}
impl<T> PartialEq for AssetPath<T> {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}
impl<T> Eq for AssetPath<T> {}
impl<T> Hash for AssetPath<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.address.hash(state);
    }
}
impl<T> fmt::Debug for AssetPath<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.address.fmt(f)
    }
}
impl<T> FromStr for AssetPath<T> {
    type Err = AssetError;
    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}
impl<T> Serialize for AssetPath<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.address.serialize(serializer)
    }
}
impl<'de, T> Deserialize<'de> for AssetPath<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Ok(Self::from_address(AssetAddress::deserialize(deserializer)?))
    }
}
