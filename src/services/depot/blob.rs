//! Content-addressed bundle cache with streaming checks.
//!
//! A blob is written to `<cache>/sha256/<hex>.part` while its SHA-256 and
//! size are computed, and renamed to `<cache>/sha256/<hex>` only when both
//! match the signed manifest. Reading a cached blob checks it again, so a
//! file changed on disk is never installed.

use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use depot_protocol::names::Sha256Hasher;
use depot_protocol::Sha256Digest;

use super::error::DepotError;
use super::ports::DepotSource;

/// Streams one blob to disk; see the module docs.
pub struct BlobWriter {
    file: Option<fs::File>,
    partial: PathBuf,
    target: PathBuf,
    hasher: Option<Sha256Hasher>,
    written: u64,
    size: u64,
    digest: Sha256Digest,
}

impl BlobWriter {
    pub fn create(target: &Path, digest: Sha256Digest, size: u64) -> Result<Self, DepotError> {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| DepotError::io(parent.display(), err))?;
        }
        let partial = PathBuf::from(format!("{}.part", target.display()));
        let file =
            fs::File::create(&partial).map_err(|err| DepotError::io(partial.display(), err))?;
        Ok(Self {
            file: Some(file),
            partial,
            target: target.to_path_buf(),
            hasher: Some(Sha256Hasher::new()),
            written: 0,
            size,
            digest,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), DepotError> {
        let length = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        self.written = self.written.saturating_add(length);
        if self.written > self.size {
            return Err(DepotError::Bundle(format!(
                "blob {} is longer than the signed size of {} bytes",
                self.digest, self.size
            )));
        }
        if let Some(hasher) = self.hasher.as_mut() {
            hasher.update(chunk);
        }
        match self.file.as_mut() {
            Some(file) => file
                .write_all(chunk)
                .map_err(|err| DepotError::io(self.partial.display(), err)),
            None => Err(DepotError::Io("blob writer already finished".to_string())),
        }
    }

    /// Checks size and digest; only then is the blob renamed into place.
    pub fn finish(mut self) -> Result<PathBuf, DepotError> {
        if self.written != self.size {
            return Err(DepotError::Bundle(format!(
                "blob {} has {} bytes, the signed manifest says {}",
                self.digest, self.written, self.size
            )));
        }
        let actual = match self.hasher.take() {
            Some(hasher) => hasher.finish(),
            None => return Err(DepotError::Io("blob writer already finished".to_string())),
        };
        if actual != self.digest {
            return Err(DepotError::Bundle(format!(
                "SHA-256 mismatch: the signed manifest says {}, the bytes are {actual}",
                self.digest
            )));
        }
        if let Some(file) = self.file.take() {
            file.sync_all()
                .map_err(|err| DepotError::io(self.partial.display(), err))?;
        }
        fs::rename(&self.partial, &self.target)
            .map_err(|err| DepotError::io(self.target.display(), err))?;
        Ok(self.target.clone())
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        if self.file.is_some() || self.hasher.is_some() {
            let _ = fs::remove_file(&self.partial);
        }
    }
}

/// Verified bundles by SHA-256, under the host's depot directory.
#[derive(Debug, Clone)]
pub struct BlobCache {
    dir: PathBuf,
}

impl BlobCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self, digest: &Sha256Digest) -> PathBuf {
        self.dir.join("sha256").join(digest.to_hex())
    }

    /// Whether a verified copy is already here.
    pub fn contains(&self, digest: &Sha256Digest, size: u64) -> bool {
        self.read(digest, size).is_ok()
    }

    /// Downloads the blob unless a verified copy is cached. Returns whether
    /// it was downloaded.
    pub async fn ensure(
        &self,
        source: &dyn DepotSource,
        digest: &Sha256Digest,
        size: u64,
    ) -> Result<bool, DepotError> {
        if self.contains(digest, size) {
            return Ok(false);
        }
        let target = self.path(digest);
        let _ = fs::remove_file(&target);
        let mut writer = BlobWriter::create(&target, *digest, size)?;
        source.blob(digest, size, &mut writer).await?;
        writer.finish()?;
        Ok(true)
    }

    /// The cached bytes, checked against size and digest once more.
    pub fn read(&self, digest: &Sha256Digest, size: u64) -> Result<Vec<u8>, DepotError> {
        let path = self.path(digest);
        let bytes = fs::read(&path).map_err(|err| DepotError::io(path.display(), err))?;
        check_bytes(&bytes, digest, size)?;
        Ok(bytes)
    }

    /// Removes every cached blob not in `keep` (and stale partial files).
    pub fn prune(&self, keep: &HashSet<String>) -> Result<usize, DepotError> {
        let dir = self.dir.join("sha256");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(DepotError::io(dir.display(), err)),
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if keep.contains(&name) {
                continue;
            }
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Size and SHA-256 of bytes against the signed values.
pub fn check_bytes(bytes: &[u8], digest: &Sha256Digest, size: u64) -> Result<(), DepotError> {
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length != size {
        return Err(DepotError::Bundle(format!(
            "blob {digest} has {length} bytes, the signed manifest says {size}"
        )));
    }
    let actual = Sha256Digest::of(bytes);
    if actual != *digest {
        return Err(DepotError::Bundle(format!(
            "SHA-256 mismatch: the signed manifest says {digest}, the bytes are {actual}"
        )));
    }
    Ok(())
}
