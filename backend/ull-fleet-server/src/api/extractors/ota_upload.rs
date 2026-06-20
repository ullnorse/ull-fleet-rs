use anyhow::Context;
use axum::body::to_bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, header};

use crate::api::MAX_UPLOAD_BYTES;
use crate::error::AppError;

const APPLICATION_OCTET_STREAM: &str = "application/octet-stream";

pub struct OtaUpload(pub Vec<u8>);

impl<S> FromRequest<S> for OtaUpload
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        validate_content_type(request.headers())?;

        let bytes = to_bytes(request.into_body(), MAX_UPLOAD_BYTES)
            .await
            .context("failed to read upload body")?;

        if bytes.is_empty() {
            return Err(AppError::Validation("uploaded file is empty".to_string()));
        }

        Ok(Self(bytes.to_vec()))
    }
}

fn validate_content_type(headers: &HeaderMap) -> Result<(), AppError> {
    let Some(content_type) = headers.get(header::CONTENT_TYPE) else {
        return Err(AppError::UnsupportedMediaType(format!(
            "expected Content-Type: {APPLICATION_OCTET_STREAM}"
        )));
    };

    let content_type = content_type
        .to_str()
        .map_err(|_| AppError::UnsupportedMediaType("invalid Content-Type header".to_string()))?;
    let mime_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();

    if mime_type.eq_ignore_ascii_case(APPLICATION_OCTET_STREAM) {
        Ok(())
    } else {
        Err(AppError::UnsupportedMediaType(format!(
            "expected Content-Type: {APPLICATION_OCTET_STREAM}"
        )))
    }
}
