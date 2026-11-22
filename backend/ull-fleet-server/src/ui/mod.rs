//! Server-rendered web UI.

pub mod chart;

use std::collections::BTreeMap;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use maud::{DOCTYPE, Markup, html};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{self, Admin};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::fleet::{self, Device, UpdateState};
use crate::releases::{self, Release};
use crate::units;

const STYLE: &str = include_str!("../../assets/style.css");
const SCRIPT: &str = include_str!("../../assets/app.js");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(devices_page))
        .route("/devices/{id}", get(device_page))
        .route("/devices/{id}/rename", post(rename_device))
        .route("/devices/{id}/remove", post(remove_device))
        .route("/devices/{id}/deploy", post(deploy_to_device))
        .route("/devices/{id}/cancel", post(cancel_update))
        .route("/releases", get(releases_page).post(upload_release))
        .route("/releases/{id}/deploy", post(deploy_to_all))
        .route("/releases/{id}/delete", post(delete_release))
        .route("/login", get(login_page).post(login))
        .route("/logout", post(logout))
        .route(
            "/assets/style.css",
            get(|| async { asset(STYLE, "text/css; charset=utf-8") }),
        )
        .route(
            "/assets/app.js",
            get(|| async { asset(SCRIPT, "text/javascript; charset=utf-8") }),
        )
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// Percent-encodes a query value.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// "just now", "12 s ago", "5 min ago".
fn ago(ts: i64, now: i64) -> String {
    let elapsed = now - ts;
    if elapsed < 10 {
        "just now".to_string()
    } else {
        format!("{} ago", fleet::duration(elapsed))
    }
}

fn size(bytes: i64) -> String {
    format!("{} KB", (bytes + 512) / 1024)
}

/// A one-off message carried in the query string after a form post.
#[derive(Deserialize, Default)]
struct Notice {
    notice: Option<String>,
    error: Option<String>,
}

fn redirect_with(path: &str, result: Result<String, AppError>) -> Response {
    let query = match result {
        Ok(message) if message.is_empty() => String::new(),
        Ok(message) => format!("?notice={}", encode(&message)),
        Err(err) => format!("?error={}", encode(&err.public_message())),
    };
    Redirect::to(&format!("{path}{query}")).into_response()
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Nav {
    Devices,
    Releases,
    None,
}

fn layout(state: &AppState, title: &str, nav: Nav, notice: &Notice, body: Markup) -> Markup {
    let signed_in = state.config.admin_password.is_some() && nav != Nav::None;
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                meta name="color-scheme" content="light dark";
                meta name="theme-color" content="#f6f7f9" media="(prefers-color-scheme: light)";
                meta name="theme-color" content="#111315" media="(prefers-color-scheme: dark)";
                meta name="apple-mobile-web-app-capable" content="yes";
                title { (title) " · ull fleet" }
                link rel="stylesheet" href="/assets/style.css";
                script src="/assets/app.js" defer {}
            }
            body {
                header.top {
                    a.brand href="/" { "ull fleet" }
                    nav {
                        a href="/" aria-current=[(nav == Nav::Devices).then_some("page")] { "Devices" }
                        a href="/releases" aria-current=[(nav == Nav::Releases).then_some("page")] { "Releases" }
                    }
                }
                main {
                    @if let Some(message) = &notice.notice {
                        p.notice role="status" { (message) }
                    }
                    @if let Some(message) = &notice.error {
                        p.notice.error role="alert" { (message) }
                    }
                    (body)
                }
                @if signed_in {
                    footer {
                        form method="post" action="/logout" {
                            button.link type="submit" { "Sign out" }
                        }
                    }
                }
            }
        }
    }
}

fn status_dot(online: bool) -> Markup {
    html! {
        span.dot.online[online] aria-hidden="true" {}
    }
}

fn readings(device: &Device, limit: usize) -> Markup {
    html! {
        @if device.latest.is_empty() {
            span.muted { "No readings" }
        } @else {
            @for (name, value) in device.latest.iter().take(limit) {
                span.reading title=(units::label(name)) {
                    (units::number(name, *value))
                    small { (units::symbol(name)) }
                }
            }
        }
    }
}

fn update_badge(device: &Device) -> Option<Markup> {
    let version = device.target_version.as_deref().unwrap_or("");
    match device.update_state {
        UpdateState::None => None,
        UpdateState::Pending => Some(html! { span.badge { "Update queued" } }),
        UpdateState::Downloading => Some(html! { span.badge { "Updating" } }),
        UpdateState::Failed => Some(html! { span.badge.bad title=(version) { "Update failed" } }),
    }
}

// ---------------------------------------------------------------------------
// Devices

async fn devices_page(
    _: Admin,
    State(state): State<AppState>,
    Query(notice): Query<Notice>,
) -> AppResult<Markup> {
    let devices = state.db.call(|conn| fleet::list_devices(conn)).await?;
    let (time, poll) = (now(), state.poll_interval_s());
    let online = devices.iter().filter(|d| d.is_online(time, poll)).count();

    let body = html! {
        div #devices data-live {
            div.heading {
                h1 { "Devices" }
                @if !devices.is_empty() {
                    p.muted { (online) " of " (devices.len()) " online" }
                }
            }
            @if devices.is_empty() {
                div.empty {
                    p { "No devices yet." }
                    p.muted {
                        "A device appears here after its first check-in to "
                        code { "/api/v1/devices/<id>/checkin" } "."
                    }
                }
            } @else {
                ul.list {
                    @for device in &devices {
                        @let is_online = device.is_online(time, poll);
                        li {
                            a.device.offline[!is_online] href=(format!("/devices/{}", device.id)) {
                                div.row {
                                    (status_dot(is_online))
                                    span.name { (device.display_name()) }
                                    span.seen.muted { (ago(device.last_seen, time)) }
                                }
                                div.readings { (readings(device, 4)) }
                                div.row.meta {
                                    span.muted.ellipsis { (device.project) " " (device.version) }
                                    @if let Some(badge) = update_badge(device) { (badge) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(layout(&state, "Devices", Nav::Devices, &notice, body))
}

#[derive(Deserialize)]
struct DeviceQuery {
    range: Option<String>,
    notice: Option<String>,
    error: Option<String>,
}

async fn device_page(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DeviceQuery>,
) -> AppResult<Markup> {
    let range = chart::range(query.range.as_deref());
    let time = now();
    let (device, series, events, releases) = state
        .db
        .call({
            let id = id.clone();
            move |conn| {
                let device = fleet::get_device(conn, &id)?;
                let mut series = BTreeMap::new();
                for (name, _) in &device.latest {
                    let points =
                        fleet::series(conn, &id, name, time - range.seconds, range.bucket_s)?;
                    series.insert(name.clone(), points);
                }
                let events = fleet::events(conn, &id, 20)?;
                let releases: Vec<Release> = releases::list(conn)?
                    .into_iter()
                    .filter(|release| release.project == device.project)
                    .collect();
                Ok((device, series, events, releases))
            }
        })
        .await?;
    let poll = state.poll_interval_s();
    let online = device.is_online(time, poll);
    let notice = Notice {
        notice: query.notice,
        error: query.error,
    };
    let base = format!("/devices/{}", device.id);

    let body = html! {
        a.back href="/" { "← Devices" }
        div #device-head data-live {
            div.heading {
                h1.ellipsis { (device.display_name()) }
                span.status.online[online] {
                    (status_dot(online))
                    @if online { "Online" } @else { "Offline" }
                }
            }
            p.muted.subtitle { "Seen " (ago(device.last_seen, time)) " · " (device.project) }
        }

        @if !device.latest.is_empty() {
            nav.segmented aria-label="Chart range" {
                @for option in chart::RANGES {
                    a href=(format!("{base}?range={}", option.key))
                        aria-current=[(option == range).then_some("page")] { (option.label) }
                }
            }
        }
        div #device-metrics data-live {
            @for (name, value) in &device.latest {
                section.card.metric {
                    div.metric-head {
                        h2 { (units::label(name)) }
                        span.value { (units::number(name, *value)) small { (units::symbol(name)) } }
                    }
                    (chart::line_chart(name, series.get(name).map_or(&[][..], Vec::as_slice), range, time))
                }
            }
        }

        section.card {
            h2 { "Firmware" }
            div #device-firmware data-live {
                dl {
                    dt { "Version" } dd.mono { (device.version) }
                    dt { "Partition" } dd {
                        (device.partition)
                        @if !device.ota_state.is_empty() { " · " (device.ota_state) }
                    }
                    dt { "Update" } dd { (update_text(&device)) }
                    @if let Some(error) = &device.last_ota_error {
                        dt { "Last error" } dd.bad-text { (error) }
                    }
                }
            }
            @if device.update_state != UpdateState::None {
                form method="post" action=(format!("{base}/cancel")) {
                    button.secondary type="submit" { "Cancel update" }
                }
            }
            @let candidates: Vec<&Release> = releases.iter().filter(|r| r.version != device.version).collect();
            @if candidates.is_empty() {
                p.muted.small { "No other releases of " (device.project) ". Upload one on the " a href="/releases" { "Releases" } " page." }
            } @else {
                form.inline method="post" action=(format!("{base}/deploy")) {
                    label.visually-hidden for="release" { "Release" }
                    select #release name="release_id" {
                        @for release in &candidates {
                            option value=(release.id) { (release.version) }
                        }
                    }
                    button type="submit" { "Update" }
                }
            }
        }

        section.card {
            h2 { "Device" }
            dl #device-info data-live {
                dt { "ID" } dd.mono { (device.id) }
                dt { "IP address" } dd.mono { (if device.ip.is_empty() { "–" } else { &device.ip }) }
                dt { "Wi-Fi signal" } dd {
                    @match device.rssi_dbm {
                        Some(rssi) => { (rssi) " dBm · " (signal_quality(rssi)) }
                        None => { "–" }
                    }
                }
                dt { "Uptime" } dd { (fleet::duration(device.uptime_s)) }
                dt { "First seen" } dd { (ago(device.first_seen, time)) }
            }
        }

        section.card {
            h2 { "Events" }
            ol.events #device-events data-live {
                @for event in &events {
                    li {
                        span.event-time.muted { (ago(event.ts, time)) }
                        span { (event.message) }
                    }
                }
                @if events.is_empty() { li.muted { "No events" } }
            }
        }

        section.card {
            h2 { "Settings" }
            form.inline method="post" action=(format!("{base}/rename")) {
                label.visually-hidden for="name" { "Name" }
                input #name name="name" value=(device.name) placeholder="Name" maxlength="40" autocomplete="off";
                button type="submit" { "Rename" }
            }
            form method="post" action=(format!("{base}/remove"))
                data-confirm="Remove this device and its history? It reappears with an empty history if it checks in again." {
                button.danger type="submit" { "Remove device" }
            }
        }
    };
    Ok(layout(
        &state,
        device.display_name(),
        Nav::Devices,
        &notice,
        body,
    ))
}

fn update_text(device: &Device) -> Markup {
    let target = device.target_version.as_deref().unwrap_or("?");
    html! {
        @match device.update_state {
            UpdateState::None => { "Up to date" }
            UpdateState::Pending => {
                "Queued: " span.mono { (target) }
                @if device.update_attempts > 0 {
                    " (attempt " (device.update_attempts + 1) " of " (fleet::MAX_UPDATE_ATTEMPTS) ")"
                }
            }
            UpdateState::Downloading => { "Installing " span.mono { (target) } }
            UpdateState::Failed => {
                span.bad-text { "Failed " (device.update_attempts) " times: " }
                span.mono { (target) }
            }
        }
    }
}

fn signal_quality(rssi: i64) -> &'static str {
    match rssi {
        -55.. => "excellent",
        -67..=-56 => "good",
        -75..=-68 => "fair",
        _ => "weak",
    }
}

#[derive(Deserialize)]
struct RenameForm {
    name: String,
}

async fn rename_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<RenameForm>,
) -> Response {
    let path = format!("/devices/{id}");
    let result = state
        .db
        .call(move |conn| fleet::rename_device(conn, &id, &form.name))
        .await
        .map(|()| "Renamed".to_string());
    redirect_with(&path, result)
}

async fn remove_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let result = state
        .db
        .call(move |conn| fleet::remove_device(conn, &id))
        .await;
    match result {
        Ok(()) => redirect_with("/", Ok("Device removed".to_string())),
        Err(err) => redirect_with("/", Err(err)),
    }
}

#[derive(Deserialize)]
struct DeployForm {
    release_id: i64,
}

async fn deploy_to_device(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<DeployForm>,
) -> Response {
    let path = format!("/devices/{id}");
    let result = state
        .db
        .call(move |conn| releases::deploy(conn, form.release_id, Some(&[id]), now()))
        .await
        .map(|outcome| {
            if outcome.targeted.is_empty() {
                "Already on this release".to_string()
            } else {
                "Update queued; it starts at the next check-in".to_string()
            }
        });
    redirect_with(&path, result)
}

async fn cancel_update(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let path = format!("/devices/{id}");
    let result = state
        .db
        .call(move |conn| releases::cancel(conn, &id, now()))
        .await
        .map(|()| "Update cancelled".to_string());
    redirect_with(&path, result)
}

// ---------------------------------------------------------------------------
// Releases

async fn releases_page(
    _: Admin,
    State(state): State<AppState>,
    Query(notice): Query<Notice>,
) -> AppResult<Markup> {
    let (releases, coverage, devices) = state
        .db
        .call(|conn| {
            Ok((
                releases::list(conn)?,
                releases::coverage(conn)?,
                fleet::list_devices(conn)?,
            ))
        })
        .await?;
    let time = now();
    let mut by_project: BTreeMap<&str, Vec<&Release>> = BTreeMap::new();
    for release in &releases {
        by_project
            .entry(&release.project)
            .or_default()
            .push(release);
    }

    let body = html! {
        div.heading { h1 { "Releases" } }
        div #releases data-live {
            @if releases.is_empty() {
                p.muted.empty { "No releases yet." }
            }
            @for (project, list) in &by_project {
                @let total = devices.iter().filter(|d| d.project == *project).count();
                section.card {
                    div.metric-head {
                        h2 { (project) }
                        span.muted.small { (total) @if total == 1 { " device" } @else { " devices" } }
                    }
                    ul.releases {
                        @for release in list {
                            @let (running, updating) = coverage.get(&release.id).copied().unwrap_or((0, 0));
                            li {
                                div.row {
                                    span.mono.ellipsis { (release.version) }
                                    span.muted.small.nowrap { (ago(release.created_at, time)) }
                                }
                                div.muted.small {
                                    (size(release.size)) " · built " (release.build_date)
                                    @if !release.notes.is_empty() { " · " (release.notes) }
                                }
                                div.row.meta {
                                    span.small {
                                        (running) " of " (total) " running"
                                        @if updating > 0 { ", " (updating) " updating" }
                                    }
                                    div.actions {
                                        @if total > 0 && running < total as i64 {
                                            form method="post" action=(format!("/releases/{}/deploy", release.id))
                                                data-confirm=(format!("Update every {project} device to {}?", release.version)) {
                                                button.small type="submit" { "Update all" }
                                            }
                                        }
                                        form method="post" action=(format!("/releases/{}/delete", release.id))
                                            data-confirm=(format!("Delete {}?", release.version)) {
                                            button.small.secondary type="submit" { "Delete" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        section.card {
            h2 { "Upload" }
            form.stack method="post" action="/releases" enctype="multipart/form-data" {
                label {
                    span { "Image" }
                    input type="file" name="image" accept=".bin,application/octet-stream" required;
                }
                label {
                    span { "Signature" }
                    input type="text" name="signature" placeholder="Output of ull-fleet-sign sign" autocomplete="off" spellcheck="false";
                }
                label {
                    span { "Notes" }
                    input type="text" name="notes" placeholder="Optional" maxlength="200";
                }
                button type="submit" { "Upload" }
            }
            p.muted.small {
                "Build with " code { "espflash save-image" } ", sign with "
                code { "ull-fleet-sign sign <key> <image>" } ". Project and version are read from the image."
            }
        }
    };
    Ok(layout(&state, "Releases", Nav::Releases, &notice, body))
}

async fn upload_release(_: Admin, State(state): State<AppState>, multipart: Multipart) -> Response {
    let result = upload_from_form(&state, multipart).await;
    redirect_with("/releases", result)
}

async fn upload_from_form(state: &AppState, mut multipart: Multipart) -> Result<String, AppError> {
    let mut image = None;
    let mut signature = String::new();
    let mut notes = String::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::bad_request(format!("bad upload: {err}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|err| AppError::bad_request(format!("bad upload: {err}")))?;
        match name.as_str() {
            "image" => image = Some(bytes),
            "signature" => signature = String::from_utf8_lossy(&bytes).into_owned(),
            "notes" => notes = String::from_utf8_lossy(&bytes).into_owned(),
            _ => {}
        }
    }
    let image = image
        .filter(|image| !image.is_empty())
        .ok_or_else(|| AppError::bad_request("choose an image file"))?;
    let validated = releases::validate(&image, &signature, state.config.ota_public_key.as_ref())?;
    let release = releases::store(
        &state.db,
        &state.config.releases_dir(),
        image.to_vec(),
        validated,
        notes.chars().take(200).collect(),
        now(),
    )
    .await?;
    tracing::info!(project = %release.project, version = %release.version, "release uploaded");
    Ok(format!("Uploaded {} {}", release.project, release.version))
}

async fn deploy_to_all(_: Admin, State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let result = state
        .db
        .call(move |conn| releases::deploy(conn, id, None, now()))
        .await
        .map(|outcome| {
            let mut message = format!(
                "Update queued for {} {}",
                outcome.targeted.len(),
                if outcome.targeted.len() == 1 {
                    "device"
                } else {
                    "devices"
                }
            );
            if !outcome.skipped.is_empty() {
                message.push_str(&format!(
                    "; skipped {} ({})",
                    outcome.skipped.len(),
                    outcome.skipped[0].1
                ));
            }
            message
        });
    redirect_with("/releases", result)
}

async fn delete_release(_: Admin, State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let dir = state.config.releases_dir();
    let result = state
        .db
        .call(move |conn| releases::delete(conn, &dir, id))
        .await
        .map(|()| "Release deleted".to_string());
    redirect_with("/releases", result)
}

// ---------------------------------------------------------------------------
// Login

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
    error: Option<String>,
}

/// Only same-site paths, so the login form cannot redirect elsewhere.
fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(path) if path.starts_with('/') && !path.starts_with("//") && !path.contains('\\') => {
            path.to_string()
        }
        _ => "/".to_string(),
    }
}

async fn login_page(State(state): State<AppState>, Query(query): Query<LoginQuery>) -> Markup {
    let notice = Notice {
        notice: None,
        error: query.error,
    };
    let body = html! {
        section.card.login {
            h1 { "Sign in" }
            form.stack method="post" action="/login" {
                input type="hidden" name="next" value=(safe_next(query.next.as_deref()));
                label {
                    span { "Password" }
                    input type="password" name="password" autocomplete="current-password" required autofocus;
                }
                button type="submit" { "Sign in" }
            }
        }
    };
    layout(&state, "Sign in", Nav::None, &notice, body)
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
    next: Option<String>,
}

async fn login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> AppResult<Response> {
    let next = safe_next(form.next.as_deref());
    if !auth::password_matches(&state, &form.password) {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        tracing::warn!("failed login");
        return Ok(Redirect::to(&format!(
            "/login?next={}&error={}",
            encode(&next),
            encode("Wrong password")
        ))
        .into_response());
    }
    let cookie = auth::create_session(&state).await?;
    let mut response = Redirect::to(&next).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|e| AppError::Internal(e.into()))?,
    );
    Ok(response)
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let cookie = auth::end_session(&state, &headers).await?;
    let mut response = Redirect::to("/login").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|e| AppError::Internal(e.into()))?,
    );
    Ok(response)
}
