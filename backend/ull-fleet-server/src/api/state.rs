use std::sync::Arc;

use axum::extract::FromRef;

use crate::config::Config;
use crate::domain::ota::OtaService;

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    ota_service: OtaService,
}

impl AppState {
    pub fn new(config: Config, ota_service: OtaService) -> Self {
        Self {
            config: Arc::new(config),
            ota_service,
        }
    }
}

impl FromRef<AppState> for Arc<Config> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.config)
    }
}

impl FromRef<AppState> for OtaService {
    fn from_ref(state: &AppState) -> Self {
        state.ota_service.clone()
    }
}
