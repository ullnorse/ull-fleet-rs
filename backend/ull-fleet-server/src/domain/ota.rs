use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::task;
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

    pub async fn pending_update(&self) -> anyhow::Result<Option<PendingUpdate>> {
        let repository = self.repository.clone();

        run_blocking("load pending OTA image", move || {
            repository.load_pending_update()
        })
        .await
    }

    pub async fn upload(&self, bytes: Vec<u8>) -> Result<PendingUpdate, UploadUpdateError> {
        let repository = self.repository.clone();
        let storage = self.storage.clone();

        task::spawn_blocking(move || upload_sync(repository, storage, bytes))
            .await
            .context("OTA upload task panicked")?
    }

    pub async fn take_pending_update(&self) -> anyhow::Result<Option<ServedUpdate>> {
        let repository = self.repository.clone();
        let storage = self.storage.clone();

        run_blocking("serve pending OTA image", move || {
            take_pending_update_sync(repository, storage)
        })
        .await
    }
}

async fn run_blocking<T, F>(task_name: &'static str, operation: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    task::spawn_blocking(operation)
        .await
        .with_context(|| format!("{task_name} task panicked"))?
}

fn upload_sync(
    repository: PendingUpdateRepository,
    storage: UpdateStorage,
    bytes: Vec<u8>,
) -> Result<PendingUpdate, UploadUpdateError> {
    if bytes.is_empty() {
        return Err(UploadUpdateError::EmptyFile);
    }

    let timestamp = unix_timestamp_secs();
    let stored_name = format!("update-{timestamp}.bin");
    let stored_path = storage
        .store_update(&stored_name, &bytes)
        .context("failed to persist uploaded OTA image")?;

    let pending = PendingUpdate {
        filename: stored_name,
        stored_path: stored_path.to_string_lossy().into_owned(),
        size_bytes: bytes.len() as u64,
        sha256_hex: sha256_hex(&bytes),
        created_at: timestamp.to_string(),
    };

    let previous_path = match repository.replace_pending_update(&pending) {
        Ok(previous_path) => previous_path,
        Err(err) => {
            let _ = storage.remove_path(&stored_path);
            return Err(UploadUpdateError::Unknown(
                err.context("failed to replace pending OTA record"),
            ));
        }
    };

    if let Some(previous_path) = previous_path.filter(|path| path != &pending.stored_path)
        && let Err(err) = storage.remove_path(Path::new(&previous_path))
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

fn take_pending_update_sync(
    repository: PendingUpdateRepository,
    storage: UpdateStorage,
) -> anyhow::Result<Option<ServedUpdate>> {
    let Some(pending) = repository.load_pending_update()? else {
        return Ok(None);
    };

    let bytes = storage.read_update(&pending.stored_path).with_context(|| {
        format!(
            "failed to read pending OTA image at {}",
            pending.stored_path
        )
    })?;

    repository
        .clear_pending_update(&pending.stored_path)
        .context("failed to clear pending OTA record")?;

    if let Err(err) = storage.remove_path(Path::new(&pending.stored_path)) {
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
