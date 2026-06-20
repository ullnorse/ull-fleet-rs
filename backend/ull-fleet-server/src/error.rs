use anyhow::Error as AnyhowError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;
use tracing::error;

use crate::domain::ota::UploadUpdateError;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    UnsupportedMediaType(String),
    #[error(transparent)]
    Internal(#[from] AnyhowError),
}

impl From<UploadUpdateError> for AppError {
    fn from(err: UploadUpdateError) -> Self {
        match err {
            UploadUpdateError::EmptyFile => Self::Validation("uploaded file is empty".to_string()),
            UploadUpdateError::Unknown(inner) => Self::Internal(inner),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::Validation(message) => (StatusCode::BAD_REQUEST, message).into_response(),
            Self::UnsupportedMediaType(message) => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, message).into_response()
            }
            Self::Internal(err) => {
                error!(error = ?err, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "an internal error occurred",
                )
                    .into_response()
            }
        }
    }
}
