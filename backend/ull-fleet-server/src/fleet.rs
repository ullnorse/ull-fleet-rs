//! Device state: check-ins, metrics, events and update decisions.
//!
//! Everything here is synchronous and takes a connection, so handlers call it
//! through [`crate::db::Db::call`] and tests call it directly.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use ull_fleet_types::{ActiveAlert, AlertRule, EventView, Point, Range, Series, Sparkline};

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
    /// Firmware or update state changed, so release coverage did too.
    #[serde(skip)]
    pub releases_changed: bool,
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
    poll_interval_s: u64,
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
            "SELECT version, uptime_s, last_seen, target_release_id, update_state, update_attempts,
                    poll_interval_s
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
                    poll_interval_s: row.get(6)?,
                })
            },
        )
        .optional()?;

    let mut events: Vec<(&'static str, String)> = Vec::new();
    let uptime = checkin.uptime_s.min(i64::MAX as u64) as i64;
    match &previous {
        None => events.push((
            "registered",
            format!("First check-in: {} {}", checkin.project, checkin.version),
        )),
        Some(previous) => {
            let gap = now - previous.last_seen;
            if gap > offline_after_s(previous.poll_interval_s) {
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
                              update_state, update_attempts, poll_interval_s)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13, ?14)
         ON CONFLICT (id) DO UPDATE SET
             project = excluded.project, version = excluded.version,
             secure_version = excluded.secure_version, partition = excluded.partition,
             ota_state = excluded.ota_state, ip = excluded.ip, rssi_dbm = excluded.rssi_dbm,
             uptime_s = excluded.uptime_s, last_ota_error = excluded.last_ota_error,
             last_seen = excluded.last_seen, update_state = excluded.update_state,
             update_attempts = excluded.update_attempts,
             poll_interval_s = excluded.poll_interval_s",
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
            poll_interval_s,
        ],
    )?;

    events.extend(evaluate_alerts(&tx, device_id, &checkin.metrics, now)?);
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

    let releases_changed = previous.as_ref().is_none_or(|previous| {
        previous.version != checkin.version || previous.update_state != state
    });
    Ok(CheckinReply {
        update: state == UpdateState::Pending,
        poll_interval_s,
        releases_changed,
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
    pub location: String,
    pub poll_interval_s: u64,
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
    /// Latest value and time of each metric, by name.
    pub latest: Vec<(String, (f64, i64))>,
}

impl Device {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn is_online(&self, now: i64) -> bool {
        now - self.last_seen <= offline_after_s(self.poll_interval_s)
    }
}

const DEVICE_COLUMNS: &str = "d.id, d.name, d.project, d.version, d.secure_version, d.partition,
    d.ota_state, d.ip, d.rssi_dbm, d.uptime_s, d.last_ota_error, d.first_seen, d.last_seen,
    d.target_release_id, r.version, d.update_state, d.update_attempts, d.location,
    d.poll_interval_s";

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
        location: row.get(17)?,
        poll_interval_s: row.get(18)?,
        latest: Vec::new(),
    })
}

fn attach_latest(conn: &Connection, devices: &mut [Device]) -> AppResult<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT name, value, ts FROM latest_metrics WHERE device_id = ?1 ORDER BY name",
    )?;
    for device in devices {
        device.latest = stmt
            .query_map([&device.id], |row| {
                Ok((row.get(0)?, (row.get(1)?, row.get(2)?)))
            })?
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
    set_text_field(conn, id, "name", name)
}

pub fn set_location(conn: &Connection, id: &str, location: &str) -> AppResult<()> {
    set_text_field(conn, id, "location", location)
}

fn set_text_field(conn: &Connection, id: &str, column: &'static str, value: &str) -> AppResult<()> {
    let value = value.trim();
    validate_text(column, value, 40)?;
    let changed = conn.execute(
        &format!("UPDATE devices SET {column} = ?2 WHERE id = ?1"),
        params![id, value],
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

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventView> {
    Ok(EventView {
        id: row.get(0)?,
        device_id: row.get(1)?,
        ts: row.get(2)?,
        kind: row.get(3)?,
        message: row.get(4)?,
    })
}

/// Events newest first, optionally of one device and older than `before_id`.
pub fn events(
    conn: &Connection,
    device_id: Option<&str>,
    before_id: Option<i64>,
    limit: u32,
) -> AppResult<Vec<EventView>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, device_id, ts, kind, message FROM events
         WHERE (?1 IS NULL OR device_id = ?1) AND (?2 IS NULL OR id < ?2)
         ORDER BY id DESC LIMIT ?3",
    )?;
    let events = stmt
        .query_map(params![device_id, before_id, limit], event_from_row)?
        .collect::<Result<_, _>>()?;
    Ok(events)
}

/// Events with an id above `after_id`, oldest first.
pub fn events_after(conn: &Connection, after_id: i64) -> AppResult<Vec<EventView>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, device_id, ts, kind, message FROM events WHERE id > ?1 ORDER BY id LIMIT 500",
    )?;
    let events = stmt
        .query_map([after_id], event_from_row)?
        .collect::<Result<_, _>>()?;
    Ok(events)
}

pub fn last_event_id(conn: &Connection) -> AppResult<i64> {
    Ok(
        conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
            row.get(0)
        })?,
    )
}

/// One metric in buckets of `range.bucket_s()`, oldest first.
pub fn series(
    conn: &Connection,
    device_id: &str,
    metric: &str,
    range: Range,
    now: i64,
) -> AppResult<Series> {
    let bucket_s = range.bucket_s();
    let from = now - range.seconds();
    let mut stmt = conn.prepare_cached(
        "SELECT (ts / ?4) * ?4 AS bucket, AVG(value), MIN(value), MAX(value) FROM metrics
         WHERE device_id = ?1 AND name = ?2 AND ts >= ?3
         GROUP BY bucket ORDER BY bucket",
    )?;
    let points = stmt
        .query_map(params![device_id, metric, from, bucket_s], |row| {
            Ok(Point {
                t: row.get(0)?,
                avg: row.get(1)?,
                min: row.get(2)?,
                max: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(Series {
        metric: metric.to_string(),
        range,
        bucket_s,
        from,
        to: now,
        points,
    })
}

pub const SPARKLINE_BUCKET_S: i64 = 1800;

/// 24 h of every metric of every device in 30-minute buckets.
pub fn sparklines(conn: &Connection, now: i64) -> AppResult<Vec<Sparkline>> {
    let mut stmt = conn.prepare(
        "SELECT device_id, name, (ts / ?2) * ?2 AS bucket, AVG(value) FROM metrics
         WHERE ts >= ?1 GROUP BY device_id, name, bucket ORDER BY device_id, name, bucket",
    )?;
    let mut rows = stmt.query(params![now - 86_400, SPARKLINE_BUCKET_S])?;
    let mut lines: Vec<Sparkline> = Vec::new();
    while let Some(row) = rows.next()? {
        let device_id: String = row.get(0)?;
        let metric: String = row.get(1)?;
        let point: (i64, f64) = (row.get(2)?, row.get(3)?);
        match lines.last_mut() {
            Some(line) if line.device_id == device_id && line.metric == metric => {
                line.min = line.min.min(point.1);
                line.max = line.max.max(point.1);
                line.points.push(point);
            }
            _ => lines.push(Sparkline {
                device_id,
                metric,
                bucket_s: SPARKLINE_BUCKET_S,
                min: point.1,
                max: point.1,
                points: vec![point],
            }),
        }
    }
    Ok(lines)
}

/// Raw readings of a device since `from`, oldest first: `(ts, metric, value)`.
pub fn raw_metrics(
    conn: &Connection,
    device_id: &str,
    from: i64,
) -> AppResult<Vec<(i64, String, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT ts, name, value FROM metrics WHERE device_id = ?1 AND ts >= ?2 ORDER BY ts, name",
    )?;
    let rows = stmt
        .query_map(params![device_id, from], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Alerts.

pub fn alert_rules(conn: &Connection, device_id: &str) -> AppResult<Vec<AlertRule>> {
    let mut stmt = conn.prepare_cached(
        "SELECT metric, min, max FROM alert_rules WHERE device_id = ?1 ORDER BY metric",
    )?;
    let rules = stmt
        .query_map([device_id], |row| {
            Ok(AlertRule {
                metric: row.get(0)?,
                min: row.get(1)?,
                max: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rules)
}

pub fn active_alerts(conn: &Connection, device_id: &str) -> AppResult<Vec<ActiveAlert>> {
    let mut stmt = conn.prepare_cached(
        "SELECT metric, value, direction, limit_value, since FROM active_alerts
         WHERE device_id = ?1 ORDER BY metric",
    )?;
    let alerts = stmt
        .query_map([device_id], |row| {
            Ok(ActiveAlert {
                metric: row.get(0)?,
                value: row.get(1)?,
                direction: row.get(2)?,
                limit: row.get(3)?,
                since: row.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(alerts)
}

/// Replaces a device's alert rules and re-checks them against its latest
/// readings. Returns the alert events this caused.
pub fn set_alert_rules(
    conn: &mut Connection,
    device_id: &str,
    rules: &[AlertRule],
    now: i64,
) -> AppResult<()> {
    if rules.len() > MAX_METRICS_PER_CHECKIN {
        return Err(AppError::bad_request("too many alert rules"));
    }
    for rule in rules {
        validate_metric_name(&rule.metric)?;
        if rule.min.is_none() && rule.max.is_none() {
            return Err(AppError::bad_request(format!(
                "the {} alert needs a lower or an upper limit",
                crate::units::label(&rule.metric)
            )));
        }
        if [rule.min, rule.max]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite())
        {
            return Err(AppError::bad_request("alert limits must be numbers"));
        }
        if let (Some(min), Some(max)) = (rule.min, rule.max)
            && min > max
        {
            return Err(AppError::bad_request(format!(
                "the {} alert's lower limit is above its upper limit",
                crate::units::label(&rule.metric)
            )));
        }
    }
    let tx = conn.transaction()?;
    let exists: bool = tx
        .query_row("SELECT 1 FROM devices WHERE id = ?1", [device_id], |_| {
            Ok(())
        })
        .optional()?
        .is_some();
    if !exists {
        return Err(AppError::NotFound);
    }
    tx.execute("DELETE FROM alert_rules WHERE device_id = ?1", [device_id])?;
    for rule in rules {
        tx.execute(
            "INSERT INTO alert_rules (device_id, metric, min, max) VALUES (?1, ?2, ?3, ?4)",
            params![device_id, rule.metric, rule.min, rule.max],
        )?;
    }
    let latest: BTreeMap<String, f64> = {
        let mut stmt = tx.prepare("SELECT name, value FROM latest_metrics WHERE device_id = ?1")?;
        stmt.query_map([device_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    // Alerts of metrics that no longer have a rule end quietly.
    tx.execute(
        "DELETE FROM active_alerts WHERE device_id = ?1
         AND metric NOT IN (SELECT metric FROM alert_rules WHERE device_id = ?1)",
        [device_id],
    )?;
    for (kind, message) in evaluate_alerts(&tx, device_id, &latest, now)? {
        add_event(&tx, device_id, now, kind, &message)?;
    }
    tx.commit()?;
    Ok(())
}

/// Compares readings with the device's rules, updates `active_alerts`, and
/// returns events for alerts that started or ended.
fn evaluate_alerts(
    conn: &Connection,
    device_id: &str,
    readings: &BTreeMap<String, f64>,
    now: i64,
) -> AppResult<Vec<(&'static str, String)>> {
    let mut events = Vec::new();
    for rule in alert_rules(conn, device_id)? {
        let Some(&value) = readings.get(&rule.metric) else {
            continue;
        };
        let breach = match (rule.min, rule.max) {
            (Some(min), _) if value < min => Some(("below", min)),
            (_, Some(max)) if value > max => Some(("above", max)),
            _ => None,
        };
        let active: Option<String> = conn
            .query_row(
                "SELECT direction FROM active_alerts WHERE device_id = ?1 AND metric = ?2",
                params![device_id, rule.metric],
                |row| row.get(0),
            )
            .optional()?;
        let label = crate::units::label(&rule.metric);
        match (breach, active) {
            (Some((direction, limit)), active) => {
                if active.as_deref() != Some(direction) {
                    events.push((
                        "alert",
                        format!(
                            "{label} {direction} {}: {}",
                            crate::units::format(&rule.metric, limit),
                            crate::units::format(&rule.metric, value)
                        ),
                    ));
                }
                conn.execute(
                    "INSERT INTO active_alerts (device_id, metric, value, direction, limit_value, since)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (device_id, metric) DO UPDATE SET value = excluded.value,
                         since = CASE WHEN active_alerts.direction = excluded.direction
                                      THEN active_alerts.since ELSE excluded.since END,
                         direction = excluded.direction, limit_value = excluded.limit_value",
                    params![device_id, rule.metric, value, direction, limit, now],
                )?;
            }
            (None, Some(_)) => {
                conn.execute(
                    "DELETE FROM active_alerts WHERE device_id = ?1 AND metric = ?2",
                    params![device_id, rule.metric],
                )?;
                events.push((
                    "alert_cleared",
                    format!(
                        "{label} back in range: {}",
                        crate::units::format(&rule.metric, value)
                    ),
                ));
            }
            (None, None) => {}
        }
    }
    Ok(events)
}

/// Deletes metrics, events and sessions past their retention.
pub fn prune(conn: &Connection, now: i64, retention_days: u32) -> AppResult<usize> {
    let cutoff = now - i64::from(retention_days) * 86_400;
    let mut removed = conn.execute("DELETE FROM metrics WHERE ts < ?1", [cutoff])?;
    removed += conn.execute("DELETE FROM events WHERE ts < ?1", [cutoff])?;
    conn.execute("DELETE FROM sessions WHERE expires_at < ?1", [now])?;
    Ok(removed)
}
