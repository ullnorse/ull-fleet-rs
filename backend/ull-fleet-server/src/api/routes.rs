use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};

use crate::api::MAX_UPLOAD_BYTES;

use super::handlers;
use super::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::<AppState>::new()
        .merge(page_routes())
        .nest("/api", api_routes())
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
}

fn page_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::index))
        .route("/hello", get(handlers::hello))
}

fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/upload", post(handlers::upload_update))
        .route("/update", get(handlers::download_update))
}
