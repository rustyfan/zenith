use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_CODE: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST: u64 = 1024 * 1024;
static TEMP_IDS: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Compiled {
    pub bytes: Vec<u8>,
    pub diagnostics: String,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    length: u64,
    modified: u128,
}
impl Stamp {
    fn read(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            path: path.canonicalize()?,
            length: metadata.len(),
            modified: metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
        })
    }
}
#[derive(Serialize, Deserialize)]
struct Fingerprint {
    files: Vec<Stamp>,
    digest: String,
}
#[derive(Serialize, Deserialize)]
struct Input {
    path: PathBuf,
    digest: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Directory {
    path: PathBuf,
    entries: Vec<(PathBuf, bool)>,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    inputs: Vec<Input>,
    directories: Vec<Directory>,
    code_digest: String,
    diagnostics: String,
}
pub(crate) struct Cache {
    path: PathBuf,
    source: PathBuf,
    directories: Vec<Directory>,
    _lock: File,
}
pub(crate) struct Temporary(pub PathBuf);
impl Temporary {
    pub fn new(directory: &Path, extension: &str) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join(format!(
            "{}.{}.{}",
            std::process::id(),
            TEMP_IDS.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock()?;
    Ok(file)
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "shader cache input exceeds size limit"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "shader cache input grew beyond size limit"
    );
    Ok(bytes)
}
fn digest(path: &Path) -> Result<String> {
    Ok(blake3::hash(&read(path, MAX_CODE)?).to_hex().to_string())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = Temporary::new(path.parent().unwrap(), "tmp")?;
    let mut file = File::create(&temporary.0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    Ok(())
}
fn compiler_fingerprint(root: &Path, compiler: &Path) -> Result<String> {
    let compiler = compiler.canonicalize()?;
    let identity = blake3::hash(compiler.as_os_str().as_encoded_bytes()).to_hex();
    let _lock = lock(&root.join(format!("compiler-{identity}.lock")))?;
    let mut files = vec![Stamp::read(&compiler)?];
    let mut pending = Vec::new();
    for entry in fs::read_dir(compiler.parent().unwrap())? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("slang") || name.starts_with("libslang") {
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_type()?.is_file() && entry.path() != compiler {
                files.push(Stamp::read(&entry.path())?);
            }
        }
    }
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            ensure!(
                files.len() + pending.len() < 4096,
                "too many compiler files"
            );
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_type()?.is_file() {
                files.push(Stamp::read(&entry.path())?);
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let path = root.join(format!("compiler-{identity}.json"));
    if let Ok(bytes) = read(&path, MAX_MANIFEST)
        && let Ok(saved) = serde_json::from_slice::<Fingerprint>(&bytes)
        && saved.files == files
        && saved.digest.len() == 64
    {
        return Ok(saved.digest);
    }
    let mut hash = blake3::Hasher::new();
    for stamp in &files {
        hash.update(&serde_json::to_vec(stamp)?);
        hash.update_reader(File::open(&stamp.path)?)?;
        ensure!(
            Stamp::read(&stamp.path)? == *stamp,
            "shader compiler changed while fingerprinting"
        );
    }
    let digest = hash.finalize().to_hex().to_string();
    atomic_write(
        &path,
        &serde_json::to_vec(&Fingerprint {
            files,
            digest: digest.clone(),
        })?,
    )?;
    Ok(digest)
}
pub(crate) fn resolve_compiler(compiler: &Path) -> Result<PathBuf> {
    if compiler.is_file() {
        return Ok(compiler.canonicalize()?);
    }
    if compiler.components().count() == 1 {
        for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            let mut path = directory.join(compiler);
            if cfg!(windows) && path.extension().is_none() {
                path.set_extension("exe");
            }
            if path.is_file() {
                return Ok(path.canonicalize()?);
            }
        }
    }
    anyhow::bail!(
        "Slang compiler not found: {}; set ZENITH_SLANGC, SLANG_DIR or PATH",
        compiler.display()
    )
}
fn directories(root: &Path) -> Result<Vec<Directory>> {
    let mut pending = vec![root.to_owned()];
    let mut result = Vec::new();
    while let Some(path) = pending.pop() {
        ensure!(result.len() < 4096, "too many shader include directories");
        let mut entries = Vec::new();
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            let directory = entry.file_type()?.is_dir();
            if directory {
                pending.push(entry.path());
            }
            entries.push((PathBuf::from(entry.file_name()), directory));
        }
        entries.sort();
        result.push(Directory { path, entries });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}
impl Cache {
    pub fn open(root: &Path, command: &Command, source: &Path) -> Result<Self> {
        let root = root.join("v1");
        fs::create_dir_all(&root)?;
        let compiler = compiler_fingerprint(&root, Path::new(command.get_program()))?;
        let args = command
            .get_args()
            .map(|arg| arg.to_str().context("non-UTF8 shader argument"))
            .collect::<Result<Vec<_>>>()?;
        let mut environment = std::env::vars_os()
            .filter(|(name, _)| name.to_string_lossy().starts_with("SLANG_"))
            .collect::<Vec<_>>();
        environment.sort();
        let key = blake3::hash(&serde_json::to_vec(&(
            1,
            compiler,
            args,
            std::env::current_dir()?,
            environment,
        ))?)
        .to_hex();
        let _lock = lock(&root.join(format!("{key}.lock")))?;
        Ok(Self {
            path: root.join(format!("{key}.bin")),
            source: source.to_owned(),
            directories: directories(source.parent().unwrap())?,
            _lock,
        })
    }
    pub fn load(&self) -> Result<Compiled> {
        let bytes = read(&self.path, MAX_CODE + MAX_MANIFEST + 8)?;
        ensure!(bytes.len() >= 8, "truncated shader cache");
        let length = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        ensure!(
            length <= MAX_MANIFEST && 8 + length <= bytes.len() as u64,
            "invalid shader manifest length"
        );
        let boundary = 8 + length as usize;
        let manifest: Manifest = serde_json::from_slice(&bytes[8..boundary])?;
        ensure!(
            !manifest.inputs.is_empty()
                && manifest.inputs.len() <= 4096
                && manifest
                    .inputs
                    .iter()
                    .any(|input| input.path == self.source),
            "invalid shader dependencies"
        );
        ensure!(
            manifest.directories == self.directories,
            "shader include directories changed"
        );
        for input in manifest.inputs {
            ensure!(digest(&input.path)? == input.digest, "shader input changed");
        }
        let bytes = bytes[boundary..].to_vec();
        validate_code(&bytes)?;
        ensure!(
            blake3::hash(&bytes).to_hex().as_str() == manifest.code_digest,
            "shader cache integrity failure"
        );
        Ok(Compiled {
            bytes,
            diagnostics: manifest.diagnostics,
        })
    }
    pub fn depfile(&self) -> Result<Temporary> {
        Temporary::new(self.path.parent().unwrap(), "d")
    }
    pub fn store(&self, compiled: &Compiled, depfile: &Path, started: SystemTime) -> Result<()> {
        let mut paths = dependencies(std::str::from_utf8(&read(depfile, MAX_MANIFEST)?)?)?;
        paths.insert(self.source.clone());
        ensure!(
            self.directories == directories(self.source.parent().unwrap())?,
            "shader include directories changed during compilation"
        );
        let inputs = paths
            .into_iter()
            .map(|path| {
                let path = path.canonicalize()?;
                let before = Stamp::read(&path)?;
                ensure!(
                    fs::metadata(&path)?.modified()? <= started,
                    "shader input changed during compilation"
                );
                let digest = digest(&path)?;
                ensure!(
                    before == Stamp::read(&path)?,
                    "shader input changed while hashing"
                );
                Ok(Input { path, digest })
            })
            .collect::<Result<Vec<_>>>()?;
        let manifest = serde_json::to_vec(&Manifest {
            inputs,
            directories: self.directories.clone(),
            code_digest: blake3::hash(&compiled.bytes).to_hex().to_string(),
            diagnostics: compiled.diagnostics.clone(),
        })?;
        ensure!(
            manifest.len() as u64 <= MAX_MANIFEST,
            "shader manifest exceeds size limit"
        );
        let mut bytes = (manifest.len() as u64).to_le_bytes().to_vec();
        bytes.extend(manifest);
        bytes.extend(&compiled.bytes);
        atomic_write(&self.path, &bytes)
    }
}
pub(crate) fn validate_code(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() >= 20 && bytes.len() as u64 <= MAX_CODE && bytes.len().is_multiple_of(4),
        "invalid SPIR-V size"
    );
    ensure!(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()) == 0x0723_0203,
        "invalid SPIR-V magic"
    );
    Ok(())
}
fn dependencies(text: &str) -> Result<BTreeSet<PathBuf>> {
    let mut chars = text.chars().peekable();
    let mut dependencies = BTreeSet::new();
    let mut token = String::new();
    let mut target = true;
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let escaped = chars.next().context("truncated dependency escape")?;
                if escaped == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                } else if escaped != '\n' {
                    token.push(escaped);
                }
            }
            ':' if target && chars.peek().is_none_or(|ch| ch.is_whitespace()) => {
                target = false;
                token.clear();
            }
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                token.push('$');
            }
            '#' => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            ch if ch.is_whitespace() => {
                if !target && !token.is_empty() {
                    dependencies.insert(PathBuf::from(std::mem::take(&mut token)));
                }
            }
            _ => token.push(ch),
        }
    }
    ensure!(!target, "invalid dependency target");
    if !token.is_empty() {
        dependencies.insert(PathBuf::from(token));
    }
    ensure!(dependencies.len() <= 4096, "too many shader dependencies");
    Ok(dependencies)
}

#[cfg(test)]
mod tests;
