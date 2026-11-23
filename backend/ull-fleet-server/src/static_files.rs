//! The web UI, built by `scripts/build-ui.sh` into `frontend/dist` and
//! embedded in release builds (debug builds read the directory at runtime).

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

use crate::error::AppError;

#[derive(Embed)]
#[folder = "../../frontend/dist"]
struct Dist;

/// Serves a UI file, or `index.html` for any other path so client-side
/// routes like `/devices/<id>` load the app.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return AppError::NotFound.into_response();
    }
    if !path.is_empty()
        && let Some(file) = Dist::get(path)
    {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Hashed names never change content; the rest is revalidated.
        let cache = if is_hashed(path) {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_str(mime.as_ref())
                        .unwrap_or(HeaderValue::from_static("application/octet-stream")),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            ],
            file.data,
        )
            .into_response();
    }
    match Dist::get("index.html") {
        Some(file) => (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            file.data,
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "The web UI is not built. Run scripts/build-ui.sh, then rebuild the server.",
        )
            .into_response(),
    }
}

/// `app-0123456789abcdef.wasm` style names written by the build script.
fn is_hashed(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .and_then(|stem| stem.rsplit_once('-'))
        .is_some_and(|(_, hash)| hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}
