//! JSON API. Device endpoints (check-in, firmware) are open; everything else
//! needs [`Admin`].

use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::io::ReaderStream;
use ull_fleet_types::{
    DeployReply, DeployRequest, DevicePatch, DeviceView, EventView, LoginRequest, Progress, Range,
    ReleaseView, Series, SessionInfo, Skipped, Snapshot, Sparkline, StreamMessage,
};

use crate::AppState;
use crate::auth::{self, Admin};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::fleet::{self, Checkin, CheckinReply};
use crate::{releases, views};

pub const SHA256_HEADER: &str = "x-application-image-sha256";
pub const VERSION_HEADER: &str = "x-application-version";
pub const NOTES_HEADER: &str = "x-release-notes";

pub fn router() -> Router<AppState> {
    Router::new()
        // Devices.
        .route("/api/v1/devices/{id}/checkin", post(checkin))
        .route("/api/v1/devices/{id}/firmware", get(firmware))
        // Session.
        .route("/api/v1/session", get(session))
        .route("/api/v1/login", post(login))
        .route("/api/v1/logout", post(logout))
        // Admin.
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/stream", get(stream_events))
        .route("/api/v1/devices", get(list_devices))
        .route(
            "/api/v1/devices/{id}",
            get(get_device).patch(patch_device).delete(remove_device),
        )
        .route("/api/v1/devices/{id}/watch", get(watch_device))
        .route("/api/v1/devices/{id}/series", get(device_series))
        .route("/api/v1/devices/{id}/readings.csv", get(readings_csv))
        .route(
            "/api/v1/devices/{id}/update",
            post(update_device).delete(cancel_update),
        )
        .route("/api/v1/events", get(list_events))
        .route("/api/v1/sparklines", get(sparklines))
        .route("/api/v1/releases", get(list_releases).post(upload_release))
        .route(
            "/api/v1/releases/{id}",
            axum::routing::delete(delete_release),
        )
        .route("/api/v1/releases/{id}/deploy", post(deploy_release))
}

// ---------------------------------------------------------------------------
// Device protocol.

async fn checkin(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Checkin>,
) -> AppResult<Json<CheckinReply>> {
    fleet::validate_device_id(&id)?;
    body.validate()?;
    let poll = state.poll_interval_for(&id);
    let reply = {
        let id = id.clone();
        state
            .db
            .call(move |conn| fleet::checkin(conn, &id, &body, now(), poll))
            .await?
    };
    state.live.clear_progress(&id);
    state.publish_changes(&[id], reply.releases_changed).await;
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

    let total = release.size as u64;
    let mut tracker = DownloadTracker {
        state: state.clone(),
        device_id: id.clone(),
        sent: 0,
        total,
        last_report: None,
        done: false,
    };
    tracker.report();
    state.publish_changes(&[id], true).await;
    let body = stream::unfold(
        (ReaderStream::new(file), tracker),
        |(mut reader, mut tracker)| async move {
            match reader.next().await {
                Some(Ok(chunk)) => {
                    tracker.add(chunk.len());
                    Some((Ok(chunk), (reader, tracker)))
                }
                Some(Err(err)) => Some((Err(err), (reader, tracker))),
                None => {
                    tracker.finish();
                    None
                }
            }
        },
    );

    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(total));
    headers.insert(SHA256_HEADER, header_value(&release.sha256)?);
    headers.insert(
        ull_fleet_sign::SIGNATURE_HEADER,
        header_value(&release.signature)?,
    );
    headers.insert(VERSION_HEADER, header_value(&release.version)?);
    Ok(response)
}

/// Reports firmware download progress to the UI. When the download stops
/// early (the device disconnected), the progress is dropped; the device's
/// next check-in records the failed attempt.
struct DownloadTracker {
    state: AppState,
    device_id: String,
    sent: u64,
    total: u64,
    last_report: Option<Instant>,
    done: bool,
}

impl DownloadTracker {
    const REPORT_EVERY: Duration = Duration::from_millis(250);

    fn add(&mut self, bytes: usize) {
        self.sent += bytes as u64;
        // With a Content-Length, hyper stops polling after the last byte, so
        // the end of the stream is never seen: finish on the byte count.
        if self.sent >= self.total {
            self.finish();
        } else if self
            .last_report
            .is_none_or(|last| last.elapsed() >= Self::REPORT_EVERY)
        {
            self.report();
        }
    }

    fn finish(&mut self) {
        self.done = self.sent == self.total;
        self.report();
    }

    fn report(&mut self) {
        self.last_report = Some(Instant::now());
        self.state.live.set_progress(
            &self.device_id,
            Progress {
                sent: self.sent,
                total: self.total,
                done: self.done,
            },
        );
    }
}

impl Drop for DownloadTracker {
    fn drop(&mut self) {
        if !self.done {
            self.state.live.clear_progress(&self.device_id);
            let state = self.state.clone();
            let id = std::mem::take(&mut self.device_id);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { state.publish_device(&id).await });
            }
        }
    }
}

fn header_value(value: &str) -> AppResult<HeaderValue> {
    HeaderValue::from_str(value).map_err(|err| AppError::Internal(err.into()))
}

// ---------------------------------------------------------------------------
// Session.

async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<SessionInfo>> {
    Ok(Json(SessionInfo {
        auth_required: state.config.admin_password.is_some(),
        signed_in: auth::is_admin(&state, &headers).await?,
    }))
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<Response> {
    if !auth::password_matches(&state, &body.password) {
        // Slow down guessing.
        tokio::time::sleep(Duration::from_millis(500)).await;
        return Err(AppError::Unauthorized);
    }
    let cookie = auth::create_session(&state).await?;
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let cookie = auth::end_session(&state, &headers).await?;
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

// ---------------------------------------------------------------------------
// Fleet views and the live stream.

async fn snapshot(_: Admin, State(state): State<AppState>) -> AppResult<Json<Snapshot>> {
    let inner = state.clone();
    Ok(Json(
        state
            .db
            .call(move |conn| views::snapshot(conn, &inner, now()))
            .await?,
    ))
}

/// Server-Sent Events: a snapshot, then [`StreamMessage`]s as things change.
/// A client that falls too far behind is disconnected and reconnects.
async fn stream_events(
    _: Admin,
    State(state): State<AppState>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let receiver = state.live.subscribe();
    let inner = state.clone();
    let snapshot = state
        .db
        .call(move |conn| views::snapshot(conn, &inner, now()))
        .await?;
    let first = serde_json::to_string(&StreamMessage::Snapshot(Box::new(snapshot)))
        .map_err(|err| AppError::Internal(err.into()))?;
    let updates = BroadcastStream::new(receiver)
        .take_while(|message| std::future::ready(message.is_ok()))
        .filter_map(|message| std::future::ready(message.ok()))
        .map(|json| Ok(Event::default().data(&*json)));
    let events = stream::once(std::future::ready(Ok(Event::default().data(first)))).chain(updates);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Held open by a device page: while it is, the device checks in every
/// `LIVE_POLL_INTERVAL_S` instead of `POLL_INTERVAL_S`.
async fn watch_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    fleet::validate_device_id(&id)?;
    let guard = state.watch(&id);
    let ticks = futures_util::stream::pending::<Result<Event, Infallible>>().map(move |event| {
        let _guard = &guard;
        event
    });
    let first = stream::once(std::future::ready(Ok(Event::default().comment("watching"))));
    Ok(Sse::new(first.chain(ticks)).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

async fn list_devices(_: Admin, State(state): State<AppState>) -> AppResult<Json<Vec<DeviceView>>> {
    let inner = state.clone();
    Ok(Json(
        state
            .db
            .call(move |conn| views::devices(conn, &inner.live, now()))
            .await?,
    ))
}

async fn get_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<DeviceView>> {
    let inner = state.clone();
    Ok(Json(
        state
            .db
            .call(move |conn| views::load_device(conn, &id, &inner.live, now()))
            .await?,
    ))
}

async fn patch_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<DevicePatch>,
) -> AppResult<Json<DeviceView>> {
    let inner = state.clone();
    let view = {
        let id = id.clone();
        state
            .db
            .call(move |conn| {
                if let Some(name) = &patch.name {
                    fleet::rename_device(conn, &id, name)?;
                }
                if let Some(location) = &patch.location {
                    fleet::set_location(conn, &id, location)?;
                }
                if let Some(rules) = &patch.alert_rules {
                    fleet::set_alert_rules(conn, &id, rules, now())?;
                }
                views::load_device(conn, &id, &inner.live, now())
            })
            .await?
    };
    state.publish_changes(&[id], false).await;
    Ok(Json(view))
}

async fn remove_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    {
        let id = id.clone();
        state
            .db
            .call(move |conn| fleet::remove_device(conn, &id))
            .await?;
    }
    state.publish_changes(&[id], true).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SeriesQuery {
    metric: String,
    range: Option<String>,
}

async fn device_series(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SeriesQuery>,
) -> AppResult<Json<Series>> {
    fleet::validate_metric_name(&query.metric)?;
    let range = parse_range(query.range.as_deref())?;
    Ok(Json(
        state
            .db
            .call(move |conn| fleet::series(conn, &id, &query.metric, range, now()))
            .await?,
    ))
}

fn parse_range(value: Option<&str>) -> AppResult<Range> {
    match value {
        None => Ok(Range::Day),
        Some(value) => Range::parse(value)
            .ok_or_else(|| AppError::bad_request("range must be one of 1h, 6h, 24h, 7d, 30d")),
    }
}

#[derive(Deserialize)]
struct CsvQuery {
    range: Option<String>,
}

async fn readings_csv(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CsvQuery>,
) -> AppResult<Response> {
    fleet::validate_device_id(&id)?;
    let range = parse_range(query.range.as_deref())?;
    let rows = {
        let id = id.clone();
        state
            .db
            .call(move |conn| {
                fleet::get_device(conn, &id)?;
                fleet::raw_metrics(conn, &id, now() - range.seconds())
            })
            .await?
    };
    let mut csv = String::from("time_utc,unix_s,metric,value\n");
    for (ts, metric, value) in rows {
        csv.push_str(&format!("{},{ts},{metric},{value}\n", iso_utc(ts)));
    }
    let disposition = format!("attachment; filename=\"{id}-{}.csv\"", range.as_str());
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        csv,
    )
        .into_response())
}

/// `2026-11-23T01:02:03Z`.
fn iso_utc(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[derive(Deserialize)]
struct UpdateRequest {
    release_id: i64,
}

async fn update_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> AppResult<Json<DeployReply>> {
    let devices = vec![id.clone()];
    let outcome = state
        .db
        .call(move |conn| releases::deploy(conn, request.release_id, Some(&devices), now()))
        .await?;
    state.publish_changes(&[id], true).await;
    Ok(Json(deploy_reply(outcome)))
}

async fn cancel_update(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    {
        let id = id.clone();
        state
            .db
            .call(move |conn| releases::cancel(conn, &id, now()))
            .await?;
    }
    state.publish_changes(&[id], true).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct EventsQuery {
    device: Option<String>,
    before: Option<i64>,
    limit: Option<u32>,
}

async fn list_events(
    _: Admin,
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> AppResult<Json<Vec<EventView>>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    Ok(Json(
        state
            .db
            .call(move |conn| fleet::events(conn, query.device.as_deref(), query.before, limit))
            .await?,
    ))
}

async fn sparklines(_: Admin, State(state): State<AppState>) -> AppResult<Json<Vec<Sparkline>>> {
    Ok(Json(
        state.db.call(|conn| fleet::sparklines(conn, now())).await?,
    ))
}

// ---------------------------------------------------------------------------
// Releases.

async fn list_releases(
    _: Admin,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ReleaseView>>> {
    Ok(Json(state.db.call(|conn| views::releases(conn)).await?))
}

#[derive(Deserialize, Default)]
struct UploadQuery {
    notes: Option<String>,
}

/// Raw image body with the signature in `X-Application-Image-Ed25519`.
/// Notes come from `X-Release-Notes` or the `notes` query parameter.
async fn upload_release(
    _: Admin,
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<(StatusCode, Json<ReleaseView>)> {
    let signature = headers
        .get(ull_fleet_sign::SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let notes = query.notes.unwrap_or_else(|| {
        headers
            .get(NOTES_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    });
    if notes.len() > 500 {
        return Err(AppError::bad_request(
            "notes must be at most 500 characters",
        ));
    }
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
    let id = release.id;
    let view = state
        .db
        .call(move |conn| {
            views::releases(conn)?
                .into_iter()
                .find(|release| release.id == id)
                .ok_or(AppError::NotFound)
        })
        .await?;
    state.publish_releases().await;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn deploy_release(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<DeployRequest>>,
) -> AppResult<Json<DeployReply>> {
    let devices = body.and_then(|Json(body)| body.devices);
    let outcome = state
        .db
        .call(move |conn| releases::deploy(conn, id, devices.as_deref(), now()))
        .await?;
    let touched: Vec<String> = outcome
        .targeted
        .iter()
        .chain(&outcome.already_current)
        .cloned()
        .collect();
    state.publish_changes(&touched, true).await;
    Ok(Json(deploy_reply(outcome)))
}

fn deploy_reply(outcome: releases::DeployOutcome) -> DeployReply {
    DeployReply {
        targeted: outcome.targeted,
        already_current: outcome.already_current,
        skipped: outcome
            .skipped
            .into_iter()
            .map(|(device, reason)| Skipped { device, reason })
            .collect(),
    }
}

async fn delete_release(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let dir = state.config.releases_dir();
    state
        .db
        .call(move |conn| releases::delete(conn, &dir, id))
        .await?;
    state.publish_releases().await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_dates() {
        assert_eq!(super::iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::iso_utc(1_795_389_495), "2026-11-22T23:18:15Z");
        assert_eq!(super::iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }
}
