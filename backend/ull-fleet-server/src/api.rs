//! JSON API. Device endpoints are open; release and deploy endpoints need [`Admin`].

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::AppState;
use crate::auth::Admin;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::fleet::{self, Checkin, CheckinReply};
use crate::releases::{self, Release};

pub const SHA256_HEADER: &str = "x-application-image-sha256";
pub const VERSION_HEADER: &str = "x-application-version";
pub const NOTES_HEADER: &str = "x-release-notes";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/devices", get(list_devices))
        .route("/api/v1/devices/{id}/checkin", post(checkin))
        .route("/api/v1/devices/{id}/firmware", get(firmware))
        .route("/api/v1/releases", get(list_releases).post(upload_release))
        .route("/api/v1/releases/{id}/deploy", post(deploy_release))
}

async fn checkin(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Checkin>,
) -> AppResult<Json<CheckinReply>> {
    fleet::validate_device_id(&id)?;
    body.validate()?;
    let poll = state.poll_interval_s();
    let reply = state
        .db
        .call(move |conn| fleet::checkin(conn, &id, &body, now(), poll))
        .await?;
    Ok(Json(reply))
}

async fn firmware(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<Response> {
    fleet::validate_device_id(&id)?;
    let release = {
        let id = id.clone();
        state
            .db
            .call(move |conn| fleet::firmware_for(conn, &id))
            .await?
    };
    let Some(release) = release else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let path = releases::image_path(&state.config.releases_dir(), &release);
    let file = tokio::fs::File::open(&path).await.map_err(|err| {
        AppError::Internal(
            anyhow::Error::from(err).context(format!("missing image {}", path.display())),
        )
    })?;
    tracing::info!(device = %id, version = %release.version, "serving firmware");

    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(release.size as u64),
    );
    headers.insert(SHA256_HEADER, header_value(&release.sha256)?);
    headers.insert(
        ull_fleet_sign::SIGNATURE_HEADER,
        header_value(&release.signature)?,
    );
    headers.insert(VERSION_HEADER, header_value(&release.version)?);
    Ok(response)
}

fn header_value(value: &str) -> AppResult<HeaderValue> {
    HeaderValue::from_str(value).map_err(|err| AppError::Internal(err.into()))
}

#[derive(Serialize)]
struct DeviceSummary {
    id: String,
    name: String,
    online: bool,
    project: String,
    version: String,
    last_seen: i64,
    update_state: &'static str,
    target_version: Option<String>,
    metrics: std::collections::BTreeMap<String, f64>,
}

async fn list_devices(
    _: Admin,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<DeviceSummary>>> {
    let devices = state.db.call(|conn| fleet::list_devices(conn)).await?;
    let (time, poll) = (now(), state.poll_interval_s());
    Ok(Json(
        devices
            .into_iter()
            .map(|device| DeviceSummary {
                online: device.is_online(time, poll),
                update_state: device.update_state.as_str(),
                metrics: device.latest.iter().cloned().collect(),
                id: device.id,
                name: device.name,
                project: device.project,
                version: device.version,
                last_seen: device.last_seen,
                target_version: device.target_version,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct ReleaseSummary {
    id: i64,
    project: String,
    version: String,
    secure_version: i64,
    size: i64,
    sha256: String,
    created_at: i64,
}

impl From<Release> for ReleaseSummary {
    fn from(release: Release) -> Self {
        Self {
            id: release.id,
            project: release.project,
            version: release.version,
            secure_version: release.secure_version,
            size: release.size,
            sha256: release.sha256,
            created_at: release.created_at,
        }
    }
}

async fn list_releases(
    _: Admin,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ReleaseSummary>>> {
    let releases = state.db.call(|conn| releases::list(conn)).await?;
    Ok(Json(releases.into_iter().map(Into::into).collect()))
}

/// Raw image body with the signature in `X-Application-Image-Ed25519`.
async fn upload_release(
    _: Admin,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<(StatusCode, Json<ReleaseSummary>)> {
    let signature = headers
        .get(ull_fleet_sign::SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let notes = headers
        .get(NOTES_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let validated = releases::validate(&body, signature, state.config.ota_public_key.as_ref())?;
    let release = releases::store(
        &state.db,
        &state.config.releases_dir(),
        body.to_vec(),
        validated,
        notes,
        now(),
    )
    .await?;
    tracing::info!(project = %release.project, version = %release.version, "release uploaded");
    Ok((StatusCode::CREATED, Json(release.into())))
}

#[derive(Deserialize, Default)]
struct DeployRequest {
    /// Devices to update; all devices of the release's project when absent.
    devices: Option<Vec<String>>,
}

#[derive(Serialize)]
struct DeployReply {
    targeted: Vec<String>,
    already_current: Vec<String>,
    skipped: Vec<Skipped>,
}

#[derive(Serialize)]
struct Skipped {
    device: String,
    reason: String,
}

async fn deploy_release(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<DeployRequest>>,
) -> AppResult<Json<DeployReply>> {
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let outcome = state
        .db
        .call(move |conn| releases::deploy(conn, id, request.devices.as_deref(), now()))
        .await?;
    Ok(Json(DeployReply {
        targeted: outcome.targeted,
        already_current: outcome.already_current,
        skipped: outcome
            .skipped
            .into_iter()
            .map(|(device, reason)| Skipped { device, reason })
            .collect(),
    }))
}
