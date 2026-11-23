//! Builds the JSON views of [`ull_fleet_types`] from the database.

use rusqlite::Connection;
use ull_fleet_types::{
    DeviceView, Reading, ReleaseView, ServerInfo, Snapshot, UpdateState as UpdateStateView,
    UpdateView,
};

use crate::error::AppResult;
use crate::fleet::{self, Device, MAX_UPDATE_ATTEMPTS, UpdateState};
use crate::live::Live;
use crate::{AppState, releases};

/// Events included in a snapshot.
pub const SNAPSHOT_EVENTS: u32 = 50;

pub fn device_view(
    conn: &Connection,
    device: Device,
    live: &Live,
    now: i64,
) -> AppResult<DeviceView> {
    let alert_rules = fleet::alert_rules(conn, &device.id)?;
    let alerts = fleet::active_alerts(conn, &device.id)?;
    let online = device.is_online(now);
    let state = match device.update_state {
        UpdateState::None => UpdateStateView::None,
        UpdateState::Pending => UpdateStateView::Pending,
        UpdateState::Downloading => UpdateStateView::Downloading,
        UpdateState::Failed => UpdateStateView::Failed,
    };
    let progress = match state {
        UpdateStateView::Downloading => live.progress(&device.id),
        _ => None,
    };
    Ok(DeviceView {
        live: live.is_watched(&device.id),
        readings: device
            .latest
            .iter()
            .map(|(name, (value, ts))| Reading {
                name: name.clone(),
                value: *value,
                ts: *ts,
            })
            .collect(),
        update: UpdateView {
            state,
            release_id: device.target_release_id,
            target_version: device.target_version,
            attempts: device.update_attempts,
            max_attempts: MAX_UPDATE_ATTEMPTS,
            progress,
        },
        online,
        alert_rules,
        alerts,
        id: device.id,
        name: device.name,
        location: device.location,
        project: device.project,
        version: device.version,
        secure_version: device.secure_version,
        partition: device.partition,
        ota_state: device.ota_state,
        ip: device.ip,
        rssi_dbm: device.rssi_dbm,
        uptime_s: device.uptime_s,
        last_ota_error: device.last_ota_error,
        reset_reason: device.reset_reason,
        reset_detail: device.reset_detail,
        first_seen: device.first_seen,
        last_seen: device.last_seen,
        poll_interval_s: device.poll_interval_s,
    })
}

pub fn load_device(conn: &Connection, id: &str, live: &Live, now: i64) -> AppResult<DeviceView> {
    let device = fleet::get_device(conn, id)?;
    device_view(conn, device, live, now)
}

pub fn devices(conn: &Connection, live: &Live, now: i64) -> AppResult<Vec<DeviceView>> {
    fleet::list_devices(conn)?
        .into_iter()
        .map(|device| device_view(conn, device, live, now))
        .collect()
}

pub fn releases(conn: &Connection) -> AppResult<Vec<ReleaseView>> {
    let coverage = releases::coverage(conn)?;
    Ok(releases::list(conn)?
        .into_iter()
        .map(|release| {
            let (running, updating) = coverage.get(&release.id).copied().unwrap_or((0, 0));
            ReleaseView {
                id: release.id,
                project: release.project,
                version: release.version,
                secure_version: release.secure_version,
                size: release.size,
                sha256: release.sha256,
                build_date: release.build_date,
                notes: release.notes,
                created_at: release.created_at,
                running,
                updating,
            }
        })
        .collect())
}

pub fn server_info(state: &AppState, now: i64) -> ServerInfo {
    let config = &state.config;
    ServerInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        poll_interval_s: config.poll_interval.as_secs(),
        live_poll_interval_s: config.live_poll_interval.as_secs(),
        retention_days: config.metrics_retention_days,
        auth_required: config.admin_password.is_some(),
        signing_key: config
            .ota_public_key
            .map(|key| ull_fleet_sign::to_hex(&key)),
        now,
    }
}

pub fn snapshot(conn: &Connection, state: &AppState, now: i64) -> AppResult<Snapshot> {
    Ok(Snapshot {
        server: server_info(state, now),
        devices: devices(conn, &state.live, now)?,
        releases: releases(conn)?,
        events: fleet::events(conn, None, None, SNAPSHOT_EVENTS)?,
    })
}
