//! Local fleet server for ull-esp-rs devices: device check-ins, metrics,
//! signed OTA releases and a phone-first web UI.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod fleet;
pub mod live;
pub mod releases;
pub mod static_files;
pub mod views;
pub use ull_fleet_types::units;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::extract::DefaultBodyLimit;

use config::Config;
use db::Db;

/// Largest accepted release image. ESP32 app partitions are at most a few MB.
pub const MAX_IMAGE_SIZE: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub live: Arc<live::Live>,
}

impl AppState {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.data_dir)
            .with_context(|| format!("failed to create {}", config.data_dir.display()))?;
        std::fs::create_dir_all(config.releases_dir())
            .with_context(|| format!("failed to create {}", config.releases_dir().display()))?;
        let db = Db::open(&config.database_path())?;
        Ok(Self {
            db,
            config: Arc::new(config),
            live: Arc::default(),
        })
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(api::router())
        .fallback(static_files::serve)
        // Skips event streams and small bodies.
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(DefaultBodyLimit::max(MAX_IMAGE_SIZE + 64 * 1024))
        .with_state(state)
}

/// Prunes old metrics, events and sessions every hour.
pub fn spawn_maintenance(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let days = state.config.metrics_retention_days;
            match state
                .db
                .call(move |conn| fleet::prune(conn, db::now(), days))
                .await
            {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "pruned old metrics and events"),
                Err(err) => tracing::warn!(error = %err, "pruning failed"),
            }
        }
    });
}
