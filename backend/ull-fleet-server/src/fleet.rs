//! Device state: check-ins, metrics, events and update decisions.
//!
//! Everything here is synchronous and takes a connection, so handlers call it
//! through [`crate::db::Db::call`] and tests call it directly.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const MAX_METRICS_PER_CHECKIN: usize = 16;
/// Failed attempts after which a release is no longer offered to a device.
pub const MAX_UPDATE_ATTEMPTS: i64 = 3;

#[derive(Debug, Deserialize)]
pub struct Checkin {
    pub project: String,
    pub version: String,
    #[serde(default)]
    pub secure_version: u32,
    #[serde(default)]
    pub partition: String,
    #[serde(default)]
    pub ota_state: String,
    #[serde(default)]
    pub uptime_s: u64,
    pub rssi_dbm: Option<i32>,
    #[serde(default)]
    pub ip: String,
    pub last_ota_error: Option<String>,
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct CheckinReply {
    pub update: bool,
    pub poll_interval_s: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// No update in progress (the device is on its target, or has none).
    None,
    /// The device will be offered the target release on its next check-in.
    Pending,
    /// The device fetched the image; the next check-in shows the outcome.
    Downloading,
    /// Gave up after [`MAX_UPDATE_ATTEMPTS`] failures.
    Failed,
}

impl UpdateState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "downloading" => Self::Downloading,
            "failed" => Self::Failed,
            _ => Self::None,
        }
    }
}

pub fn validate_device_id(id: &str) -> AppResult<()> {
    if id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "device id must be 12 lowercase hex digits (the Wi-Fi MAC address)",
        ))
    }
}

pub fn validate_metric_name(name: &str) -> AppResult<()> {
    if (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        Ok(())
    } else {
        Err(AppError::bad_request(format!(
            "invalid metric name {name:?}: use 1-32 characters of a-z, 0-9 and _"
        )))
    }
}

fn validate_text(field: &str, value: &str, max: usize) -> AppResult<()> {
    if value.len() > max || value.chars().any(char::is_control) {
        Err(AppError::bad_request(format!(
            "{field} must be at most {max} printable characters"
        )))
    } else {
        Ok(())
    }
}

impl Checkin {
    pub fn validate(&self) -> AppResult<()> {
        validate_text("project", &self.project, 32)?;
        validate_text("version", &self.version, 32)?;
        if self.project.is_empty() || self.version.is_empty() {
            return Err(AppError::bad_request("project and version are required"));
        }
        validate_text("partition", &self.partition, 16)?;
        validate_text("ota_state", &self.ota_state, 16)?;
        validate_text("ip", &self.ip, 45)?;
        if let Some(error) = &self.last_ota_error {
            validate_text("last_ota_error", error, 200)?;
        }
        if self.metrics.len() > MAX_METRICS_PER_CHECKIN {
            return Err(AppError::bad_request(format!(
                "at most {MAX_METRICS_PER_CHECKIN} metrics per check-in"
            )));
        }
        for (name, value) in &self.metrics {
            validate_metric_name(name)?;
            if !value.is_finite() {
                return Err(AppError::bad_request(format!(
                    "metric {name} is not finite"
                )));
            }
        }
        Ok(())
    }
}

struct Previous {
    version: String,
    uptime_s: i64,
    last_seen: i64,
    target_release_id: Option<i64>,
    update_state: UpdateState,
    update_attempts: i64,
}

/// Records a check-in and decides whether the device should update.
pub fn checkin(
    conn: &mut Connection,
    device_id: &str,
    checkin: &Checkin,
    now: i64,
    poll_interval_s: u64,
) -> AppResult<CheckinReply> {
    let tx = conn.transaction()?;
    let previous = tx
        .query_row(
            "SELECT version, uptime_s, last_seen, target_release_id, update_state, update_attempts
             FROM devices WHERE id = ?1",
            [device_id],
            |row| {
                Ok(Previous {
                    version: row.get(0)?,
                    uptime_s: row.get(1)?,
                    last_seen: row.get(2)?,
                    target_release_id: row.get(3)?,
                    update_state: UpdateState::parse(&row.get::<_, String>(4)?),
                    update_attempts: row.get(5)?,
                })
            },
        )
        .optional()?;

    let mut events: Vec<(&str, String)> = Vec::new();
    let uptime = checkin.uptime_s.min(i64::MAX as u64) as i64;
    match &previous {
        None => events.push((
            "registered",
            format!("First check-in: {} {}", checkin.project, checkin.version),
        )),
        Some(previous) => {
            let gap = now - previous.last_seen;
            if gap > offline_after_s(poll_interval_s) {
                events.push(("online", format!("Back online after {}", duration(gap))));
            }
            if uptime < previous.uptime_s {
                events.push(("restarted", "Restarted".to_string()));
            }
            if previous.version != checkin.version {
                events.push((
                    "firmware",
                    format!("Firmware {} → {}", previous.version, checkin.version),
                ));
            }
        }
    }

    // Update bookkeeping against the device's target release.
    let (mut state, mut attempts) = previous.as_ref().map_or((UpdateState::None, 0), |p| {
        (p.update_state, p.update_attempts)
    });
    let target_version: Option<String> = match previous.as_ref().and_then(|p| p.target_release_id) {
        Some(id) => tx
            .query_row("SELECT version FROM releases WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?,
        None => None,
    };
    match &target_version {
        Some(target) if *target == checkin.version => {
            if state != UpdateState::None {
                events.push(("update", format!("Updated to {target}")));
            }
            state = UpdateState::None;
            attempts = 0;
        }
        Some(target) => {
            if state == UpdateState::Downloading {
                attempts += 1;
                let reason = checkin
                    .last_ota_error
                    .clone()
                    .unwrap_or_else(|| format!("still on {}", checkin.version));
                events.push((
                    "update_failed",
                    format!(
                        "Update to {target} failed ({attempts}/{MAX_UPDATE_ATTEMPTS}): {reason}"
                    ),
                ));
                state = if attempts >= MAX_UPDATE_ATTEMPTS {
                    UpdateState::Failed
                } else {
                    UpdateState::Pending
                };
            }
        }
        None => state = UpdateState::None,
    }

    tx.execute(
        "INSERT INTO devices (id, project, version, secure_version, partition, ota_state, ip,
                              rssi_dbm, uptime_s, last_ota_error, first_seen, last_seen,
                              update_state, update_attempts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13)
         ON CONFLICT (id) DO UPDATE SET
             project = excluded.project, version = excluded.version,
             secure_version = excluded.secure_version, partition = excluded.partition,
             ota_state = excluded.ota_state, ip = excluded.ip, rssi_dbm = excluded.rssi_dbm,
             uptime_s = excluded.uptime_s, last_ota_error = excluded.last_ota_error,
             last_seen = excluded.last_seen, update_state = excluded.update_state,
             update_attempts = excluded.update_attempts",
        params![
            device_id,
            checkin.project,
            checkin.version,
            checkin.secure_version,
            checkin.partition,
            checkin.ota_state,
            checkin.ip,
            checkin.rssi_dbm,
            uptime,
            checkin.last_ota_error,
            now,
            state.as_str(),
            attempts,
        ],
    )?;

    for (kind, message) in events {
        add_event(&tx, device_id, now, kind, &message)?;
    }
    for (name, value) in &checkin.metrics {
        tx.execute(
            "INSERT INTO metrics (device_id, name, ts, value) VALUES (?1, ?2, ?3, ?4)",
            params![device_id, name, now, value],
        )?;
        tx.execute(
            "INSERT INTO latest_metrics (device_id, name, ts, value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (device_id, name) DO UPDATE SET ts = excluded.ts, value = excluded.value",
            params![device_id, name, now, value],
        )?;
    }
    tx.commit()?;

    Ok(CheckinReply {
        update: state == UpdateState::Pending,
        poll_interval_s,
    })
}

pub fn add_event(
    conn: &Connection,
    device_id: &str,
    ts: i64,
    kind: &str,
    message: &str,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO events (device_id, ts, kind, message) VALUES (?1, ?2, ?3, ?4)",
        params![device_id, ts, kind, message],
    )?;
    Ok(())
}

/// The release a device should download, if any. Marks it as downloading.
pub fn firmware_for(
    conn: &Connection,
    device_id: &str,
) -> AppResult<Option<crate::releases::Release>> {
    let row: Option<(Option<i64>, String, String)> = conn
        .query_row(
            "SELECT target_release_id, update_state, version FROM devices WHERE id = ?1",
            [device_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((target, state, version)) = row else {
        return Err(AppError::NotFound);
    };
    let state = UpdateState::parse(&state);
    if !matches!(state, UpdateState::Pending | UpdateState::Downloading) {
        return Ok(None);
    }
    let Some(release) = target
        .map(|id| crate::releases::get(conn, id))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    if release.version == version {
        return Ok(None);
    }
    conn.execute(
        "UPDATE devices SET update_state = 'downloading' WHERE id = ?1",
        [device_id],
    )?;
    Ok(Some(release))
}

pub fn offline_after_s(poll_interval_s: u64) -> i64 {
    (poll_interval_s as i64 * 3).max(60)
}

/// A short human duration: "45 s", "12 min", "3 h", "2 d".
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        3600..86400 => format!("{} h", seconds / 3600),
        _ => format!("{} d", seconds / 86400),
    }
}

// ---------------------------------------------------------------------------
// Queries for the UI.

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub project: String,
    pub version: String,
    pub secure_version: i64,
    pub partition: String,
    pub ota_state: String,
    pub ip: String,
    pub rssi_dbm: Option<i64>,
    pub uptime_s: i64,
    pub last_ota_error: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub target_release_id: Option<i64>,
    pub target_version: Option<String>,
    pub update_state: UpdateState,
    pub update_attempts: i64,
    /// Latest value of each metric, by name.
    pub latest: Vec<(String, f64)>,
}

impl Device {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn is_online(&self, now: i64, poll_interval_s: u64) -> bool {
        now - self.last_seen <= offline_after_s(poll_interval_s)
    }
}

const DEVICE_COLUMNS: &str = "d.id, d.name, d.project, d.version, d.secure_version, d.partition,
    d.ota_state, d.ip, d.rssi_dbm, d.uptime_s, d.last_ota_error, d.first_seen, d.last_seen,
    d.target_release_id, r.version, d.update_state, d.update_attempts";

fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: row.get(0)?,
        name: row.get(1)?,
        project: row.get(2)?,
        version: row.get(3)?,
        secure_version: row.get(4)?,
        partition: row.get(5)?,
        ota_state: row.get(6)?,
        ip: row.get(7)?,
        rssi_dbm: row.get(8)?,
        uptime_s: row.get(9)?,
        last_ota_error: row.get(10)?,
        first_seen: row.get(11)?,
        last_seen: row.get(12)?,
        target_release_id: row.get(13)?,
        target_version: row.get(14)?,
        update_state: UpdateState::parse(&row.get::<_, String>(15)?),
        update_attempts: row.get(16)?,
        latest: Vec::new(),
    })
}

fn attach_latest(conn: &Connection, devices: &mut [Device]) -> AppResult<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT name, value FROM latest_metrics WHERE device_id = ?1 ORDER BY name",
    )?;
    for device in devices {
        device.latest = stmt
            .query_map([&device.id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        crate::units::sort_metrics(&mut device.latest);
    }
    Ok(())
}

pub fn list_devices(conn: &Connection) -> AppResult<Vec<Device>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices d
         LEFT JOIN releases r ON r.id = d.target_release_id
         ORDER BY d.name = '', d.name COLLATE NOCASE, d.id"
    ))?;
    let mut devices: Vec<Device> = stmt
        .query_map([], device_from_row)?
        .collect::<Result<_, _>>()?;
    attach_latest(conn, &mut devices)?;
    Ok(devices)
}

pub fn get_device(conn: &Connection, id: &str) -> AppResult<Device> {
    let mut device = conn
        .query_row(
            &format!(
                "SELECT {DEVICE_COLUMNS} FROM devices d
                 LEFT JOIN releases r ON r.id = d.target_release_id WHERE d.id = ?1"
            ),
            [id],
            device_from_row,
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    attach_latest(conn, std::slice::from_mut(&mut device))?;
    Ok(device)
}

pub fn rename_device(conn: &Connection, id: &str, name: &str) -> AppResult<()> {
    let name = name.trim();
    validate_text("name", name, 40)?;
    let changed = conn.execute(
        "UPDATE devices SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub fn remove_device(conn: &Connection, id: &str) -> AppResult<()> {
    let changed = conn.execute("DELETE FROM devices WHERE id = ?1", [id])?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Event {
    pub ts: i64,
    pub kind: String,
    pub message: String,
}

pub fn events(conn: &Connection, device_id: &str, limit: u32) -> AppResult<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT ts, kind, message FROM events WHERE device_id = ?1 ORDER BY ts DESC, id DESC LIMIT ?2",
    )?;
    let events = stmt
        .query_map(params![device_id, limit], |row| {
            Ok(Event {
                ts: row.get(0)?,
                kind: row.get(1)?,
                message: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(events)
}

/// Bucketed averages of one metric: `(bucket start, average)`, oldest first.
pub fn series(
    conn: &Connection,
    device_id: &str,
    name: &str,
    since: i64,
    bucket_s: i64,
) -> AppResult<Vec<(i64, f64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT (ts / ?4) * ?4 AS bucket, AVG(value) FROM metrics
         WHERE device_id = ?1 AND name = ?2 AND ts >= ?3
         GROUP BY bucket ORDER BY bucket",
    )?;
    let points = stmt
        .query_map(params![device_id, name, since, bucket_s], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(points)
}

/// Deletes metrics, events and sessions past their retention.
pub fn prune(conn: &Connection, now: i64, retention_days: u32) -> AppResult<usize> {
    let cutoff = now - i64::from(retention_days) * 86_400;
    let mut removed = conn.execute("DELETE FROM metrics WHERE ts < ?1", [cutoff])?;
    removed += conn.execute("DELETE FROM events WHERE ts < ?1", [cutoff])?;
    conn.execute("DELETE FROM sessions WHERE expires_at < ?1", [now])?;
    Ok(removed)
}
