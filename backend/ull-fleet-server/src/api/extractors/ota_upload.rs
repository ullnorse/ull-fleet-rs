use anyhow::Context;
use axum::body::to_bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, header};

use crate::api::MAX_UPLOAD_BYTES;
use crate::domain::ota::SignedUpload;
use crate::error::AppError;

const APPLICATION_OCTET_STREAM: &str = "application/octet-stream";

/// An image upload. The body is the raw image and the
/// `x-application-image-ed25519` header carries its signature.
pub struct OtaUpload(pub SignedUpload);

impl<S> FromRequest<S> for OtaUpload
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        validate_content_type(request.headers())?;
        let signature = signature(request.headers())?;

        let bytes = to_bytes(request.into_body(), MAX_UPLOAD_BYTES)
            .await
            .context("failed to read upload body")?;

        if bytes.is_empty() {
            return Err(AppError::Validation("uploaded file is empty".to_string()));
        }

        Ok(Self(SignedUpload {
            bytes: bytes.to_vec(),
            signature,
        }))
    }
}

fn signature(headers: &HeaderMap) -> Result<[u8; 64], AppError> {
    let header = ull_fleet_sign::SIGNATURE_HEADER;
    let value = headers.get(header).ok_or_else(|| {
        AppError::Validation(format!(
            "missing {header} header; sign the image with ull-fleet-sign"
        ))
    })?;
    value
        .to_str()
        .ok()
        .and_then(|value| ull_fleet_sign::from_hex(value).ok())
        .ok_or_else(|| AppError::Validation(format!("{header} must be 128 hex digits")))
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
