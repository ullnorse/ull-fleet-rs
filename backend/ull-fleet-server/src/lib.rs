pub mod api;
pub mod config;
pub mod domain;
pub mod error;
pub mod infra;

use anyhow::Context;
use axum::Router;
use config::Config;
use domain::ota::OtaService;
use infra::{PendingUpdateRepository, UpdateStorage};

pub fn create_app(config: Config) -> anyhow::Result<Router> {
    let repository = PendingUpdateRepository::new(config.database_path.clone());
    let storage = UpdateStorage::new(config.uploads_dir.clone());
    let ota_service = OtaService::new(repository, storage);

    ota_service
        .initialize()
        .context("failed to initialize OTA service")?;

    Ok(api::routes::router(api::state::AppState::new(
        config,
        ota_service,
    )))
}
