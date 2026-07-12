//! Content-addressed blob store under `$data_dir/blobs/ca/ab/<hash>`.

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
    }
}
