use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{info, warn};

use crate::infra::{PendingUpdateRepository, UpdateStorage};

#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub filename: String,
    pub stored_path: String,
    pub size_bytes: u64,
    pub sha256_hex: String,
    pub created_at: String,
}

#[derive(Debug)]
pub struct ServedUpdate {
    pub pending: PendingUpdate,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum UploadUpdateError {
    #[error("uploaded file is empty")]
    EmptyFile,
    #[error(transparent)]
    Unknown(#[from] anyhow::Error),
}

#[derive(Clone)]
pub struct OtaService {
    repository: PendingUpdateRepository,
    storage: UpdateStorage,
}

impl OtaService {
    pub fn new(repository: PendingUpdateRepository, storage: UpdateStorage) -> Self {
        Self {
            repository,
            storage,
        }
    }

    pub fn initialize(&self) -> anyhow::Result<()> {
        self.repository
            .initialize()
            .context("failed to initialize pending update repository")?;
        self.storage
            .initialize()
            .context("failed to initialize update storage")?;
        Ok(())
    }

    pub fn pending_update(&self) -> anyhow::Result<Option<PendingUpdate>> {
        self.repository
            .load_pending_update()
            .context("failed to load pending OTA image")
    }

    pub fn upload(&self, bytes: &[u8]) -> Result<PendingUpdate, UploadUpdateError> {
        if bytes.is_empty() {
            return Err(UploadUpdateError::EmptyFile);
        }

        let timestamp = unix_timestamp_secs();
        let stored_name = format!("update-{timestamp}.bin");
        let stored_path = self
            .storage
            .store_update(&stored_name, bytes)
            .context("failed to persist uploaded OTA image")?;

        let pending = PendingUpdate {
            filename: stored_name,
            stored_path: stored_path.to_string_lossy().into_owned(),
            size_bytes: bytes.len() as u64,
            sha256_hex: sha256_hex(bytes),
            created_at: timestamp.to_string(),
        };

        let previous_path = match self.repository.replace_pending_update(&pending) {
            Ok(previous_path) => previous_path,
            Err(err) => {
                let _ = self.storage.remove_path(&stored_path);
                return Err(UploadUpdateError::Unknown(
                    err.context("failed to replace pending OTA record"),
                ));
            }
        };

        if let Some(previous_path) = previous_path.filter(|path| path != &pending.stored_path)
            && let Err(err) = self.storage.remove_path(Path::new(&previous_path))
        {
            warn!(
                path = previous_path.as_str(),
                error = %err,
                "failed to delete superseded OTA file"
            );
        }

        info!(
            filename = pending.filename.as_str(),
            size_bytes = pending.size_bytes,
            sha256 = pending.sha256_hex.as_str(),
            "stored pending OTA image"
        );

        Ok(pending)
    }

    pub fn take_pending_update(&self) -> anyhow::Result<Option<ServedUpdate>> {
        let Some(pending) = self.pending_update()? else {
            return Ok(None);
        };

        let bytes = self
            .storage
            .read_update(&pending.stored_path)
            .with_context(|| {
                format!(
                    "failed to read pending OTA image at {}",
                    pending.stored_path
                )
            })?;

        self.repository
            .clear_pending_update(&pending.stored_path)
            .context("failed to clear pending OTA record")?;

        if let Err(err) = self.storage.remove_path(Path::new(&pending.stored_path)) {
            warn!(
                path = pending.stored_path.as_str(),
                error = %err,
                "failed to delete served OTA file"
            );
        }

        info!(
            filename = pending.filename.as_str(),
            size_bytes = pending.size_bytes,
            sha256 = pending.sha256_hex.as_str(),
            "serving and deleting pending OTA image"
        );

        Ok(Some(ServedUpdate { pending, bytes }))
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);

    for byte in digest {
        use std::fmt::Write as _;

        let _ = write!(hex, "{byte:02x}");
    }

    hex
}
