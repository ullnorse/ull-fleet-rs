use std::path::{Path, PathBuf};

use anyhow::Context;

#[derive(Clone)]
pub struct UpdateStorage {
    uploads_dir: PathBuf,
}

impl UpdateStorage {
    pub fn new(uploads_dir: PathBuf) -> Self {
        Self { uploads_dir }
    }

    pub fn initialize(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.uploads_dir).with_context(|| {
            format!(
                "failed to create uploads directory {}",
                self.uploads_dir.display()
            )
        })?;
        Ok(())
    }

    pub fn store_update(&self, stored_name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
        let path = self.uploads_dir.join(stored_name);
        std::fs::write(&path, bytes)
            .with_context(|| format!("failed to write OTA image to {}", path.display()))?;
        Ok(path)
    }

    pub fn read_update(&self, path: impl AsRef<Path>) -> anyhow::Result<Vec<u8>> {
        let path = path.as_ref();
        std::fs::read(path)
            .with_context(|| format!("failed to read OTA image from {}", path.display()))
    }

    pub fn remove_path(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove OTA image at {}", path.display()))
    }
}
