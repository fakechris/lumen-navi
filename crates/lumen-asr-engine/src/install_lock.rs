use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub const SENSEVOICE_INSTALL_LOCK_NAME: &str = ".sensevoice-install.lock";

pub struct ModelInstallLock {
    file: File,
}

impl ModelInstallLock {
    pub fn try_acquire(models_root: &Path) -> io::Result<Option<Self>> {
        std::fs::create_dir_all(models_root)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(models_root.join(SENSEVOICE_INSTALL_LOCK_NAME))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Drop for ModelInstallLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::ModelInstallLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn only_one_installer_can_lock_a_shared_models_root() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lumen-install-lock-{nonce}"));

        let first = ModelInstallLock::try_acquire(&root).unwrap().unwrap();
        assert!(ModelInstallLock::try_acquire(&root).unwrap().is_none());
        drop(first);
        assert!(ModelInstallLock::try_acquire(&root).unwrap().is_some());

        let _ = std::fs::remove_dir_all(root);
    }
}
