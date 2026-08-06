//! Content-addressed blob store under `$data_dir/blobs/ca/ab/<hash>`.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use lumen_types::ArtifactRef;
use uuid::Uuid;

use crate::StoreError;

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
    tmp: PathBuf,
}

impl BlobStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = data_dir.as_ref().join("blobs");
        let tmp = data_dir.as_ref().join("tmp");
        fs::create_dir_all(&root).map_err(StoreError::io)?;
        fs::create_dir_all(&tmp).map_err(StoreError::io)?;
        Ok(Self { root, tmp })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Write bytes under content-addressed path. Returns relative path from data_dir parent
    /// style: `blobs/ca/ab/<fullhash>` relative to data_dir.
    pub fn put_bytes(&self, media_type: impl Into<String>, bytes: &[u8]) -> Result<ArtifactRef, StoreError> {
        let hash = blake3::hash(bytes);
        let hex = hash.to_hex().to_string();
        let relative = relative_blob_path(&hex);
        let absolute = self
            .root
            .parent() // data_dir
            .ok_or_else(|| StoreError::Other("blob root has no parent".into()))?
            .join(&relative);

        if !absolute.exists() {
            if let Some(parent) = absolute.parent() {
                fs::create_dir_all(parent).map_err(StoreError::io)?;
            }
            let tmp_name = format!("{}.{}.part", hex, Uuid::new_v4());
            let tmp_path = self.tmp.join(tmp_name);
            {
                let mut f = fs::File::create(&tmp_path).map_err(StoreError::io)?;
                f.write_all(bytes).map_err(StoreError::io)?;
                f.sync_all().map_err(StoreError::io)?;
            }
            fs::rename(&tmp_path, &absolute).map_err(StoreError::io)?;
        }

        Ok(ArtifactRef {
            id: Uuid::new_v4(),
            media_type: media_type.into(),
            path: relative,
            bytes: Some(bytes.len() as u64),
            content_hash: Some(hex),
        })
    }

    pub fn read_relative(&self, relative: &str) -> Result<Vec<u8>, StoreError> {
        let data_dir = self
            .root
            .parent()
            .ok_or_else(|| StoreError::Other("blob root has no parent".into()))?;
        let path = data_dir.join(relative);
        fs::read(path).map_err(StoreError::io)
    }

    /// Current bytes in the content-addressed blob tree. Temporary files are
    /// excluded so an interrupted write cannot permanently disable intake.
    pub fn total_bytes(&self) -> Result<u64, StoreError> {
        directory_bytes(&self.root)
    }

    /// Bytes a set of bodies would add after content-addressed deduplication.
    pub fn additional_bytes<'a>(
        &self,
        bodies: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<u64, StoreError> {
        let data_dir = self
            .root
            .parent()
            .ok_or_else(|| StoreError::Other("blob root has no parent".into()))?;
        let mut seen = HashSet::new();
        let mut total = 0_u64;
        for bytes in bodies {
            let hex = blake3::hash(bytes).to_hex().to_string();
            if seen.insert(hex.clone()) && !data_dir.join(relative_blob_path(&hex)).exists() {
                total = total.saturating_add(bytes.len() as u64);
            }
        }
        Ok(total)
    }

    /// Remove all blob files (used by wipe). Keeps directory structure.
    pub fn wipe_all(&self) -> Result<(), StoreError> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).map_err(StoreError::io)?;
        }
        fs::create_dir_all(&self.root).map_err(StoreError::io)?;
        if self.tmp.exists() {
            fs::remove_dir_all(&self.tmp).map_err(StoreError::io)?;
        }
        fs::create_dir_all(&self.tmp).map_err(StoreError::io)?;
        Ok(())
    }
}

fn directory_bytes(path: &Path) -> Result<u64, StoreError> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path).map_err(StoreError::io)? {
        let entry = entry.map_err(StoreError::io)?;
        let metadata = entry.metadata().map_err(StoreError::io)?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_bytes(&entry.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn relative_blob_path(hex: &str) -> String {
    let a = hex.get(0..2).unwrap_or("00");
    let b = hex.get(2..4).unwrap_or("00");
    format!("blobs/{a}/{b}/{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_is_content_addressed_and_dedupes() {
        let dir = tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let a = blobs.put_bytes("image/png", b"hello").unwrap();
        let b = blobs.put_bytes("image/png", b"hello").unwrap();
        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.path, b.path);
        assert_eq!(blobs.read_relative(&a.path).unwrap(), b"hello");
        assert_eq!(blobs.total_bytes().unwrap(), 5);
        assert_eq!(blobs.additional_bytes([b"hello".as_slice()]).unwrap(), 0);
        assert_eq!(blobs.additional_bytes([b"new".as_slice(), b"new".as_slice()]).unwrap(), 3);
    }
}
