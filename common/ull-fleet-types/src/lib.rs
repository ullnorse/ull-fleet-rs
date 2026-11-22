//! JSON shapes of the ull-fleet admin API and live stream, shared by the
//! server and the web UI. Device firmware uses its own minimal protocol
//! (`/api/v1/devices/{id}/checkin`), documented in `docs/DESIGN.md`.

pub mod units;

use serde::{Deserialize, Serialize};

/// Server facts the UI needs once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    /// Check-in interval for devices nobody is watching.
    pub poll_interval_s: u64,
    /// Check-in interval while a device page is open.
    pub live_poll_interval_s: u64,
    pub retention_days: u32,
    /// Login required for the UI and admin API.
    pub auth_required: bool,
    /// Hex Ed25519 key uploads must be signed with, when configured.
    pub signing_key: Option<String>,
    /// Server clock (Unix seconds) when this was sent.
    pub now: i64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    /// No update in progress.
    None,
    /// The device gets the target on its next check-in.
    Pending,
    /// The device fetched the image; the next check-in shows the outcome.
    Downloading,
    /// Gave up after the maximum number of attempts.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateView {
    pub state: UpdateState,
    pub release_id: Option<i64>,
    pub target_version: Option<String>,
    pub attempts: i64,
    pub max_attempts: i64,
    /// Bytes served of the current download, while one is running.
    pub progress: Option<Progress>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub sent: u64,
    pub total: u64,
    /// The download finished; the device is verifying and rebooting.
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub name: String,
    pub value: f64,
    pub ts: i64,
}

/// A threshold on one metric. An alert is active while the latest value is
/// outside `[min, max]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub metric: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub metric: String,
    pub value: f64,
    /// "above" or "below".
    pub direction: String,
    pub limit: f64,
    pub since: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub location: String,
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
    pub online: bool,
    /// Interval the server last gave this device.
    pub poll_interval_s: u64,
    /// Someone has the device page open, so it checks in faster.
    pub live: bool,
    pub update: UpdateView,
    pub readings: Vec<Reading>,
    pub alert_rules: Vec<AlertRule>,
    pub alerts: Vec<ActiveAlert>,
}

impl DeviceView {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn reading(&self, name: &str) -> Option<&Reading> {
        self.readings.iter().find(|reading| reading.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventView {
    pub id: i64,
    pub device_id: String,
    pub ts: i64,
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseView {
    pub id: i64,
    pub project: String,
    pub version: String,
    pub secure_version: i64,
    pub size: i64,
    pub sha256: String,
    pub build_date: String,
    pub notes: String,
    pub created_at: i64,
    /// Devices running this release.
    pub running: i64,
    /// Devices with an update to this release in progress.
    pub updating: i64,
}

/// Everything the UI shows on its first screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub server: ServerInfo,
    pub devices: Vec<DeviceView>,
    pub releases: Vec<ReleaseView>,
    /// Newest first.
    pub events: Vec<EventView>,
}

/// One message on `/api/v1/stream` (Server-Sent Events, JSON data).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamMessage {
    /// Sent first on every connection.
    Snapshot(Snapshot),
    /// A device changed: check-in, rename, online state, update state.
    Device(DeviceView),
    DeviceRemoved { id: String },
    Event(EventView),
    /// Firmware download progress.
    Progress { device_id: String, progress: Progress },
    Releases { releases: Vec<ReleaseView> },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Range {
    #[serde(rename = "1h")]
    Hour,
    #[serde(rename = "6h")]
    SixHours,
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
}

impl Range {
    pub const ALL: [Range; 5] = [
        Range::Hour,
        Range::SixHours,
        Range::Day,
        Range::Week,
        Range::Month,
    ];

    pub fn seconds(self) -> i64 {
        match self {
            Range::Hour => 3_600,
            Range::SixHours => 6 * 3_600,
            Range::Day => 86_400,
            Range::Week => 7 * 86_400,
            Range::Month => 30 * 86_400,
        }
    }

    /// Bucket width: about 180 points per range, never finer than the
    /// fastest check-in interval.
    pub fn bucket_s(self) -> i64 {
        match self {
            Range::Hour => 20,
            Range::SixHours => 120,
            Range::Day => 480,
            Range::Week => 3_600,
            Range::Month => 4 * 3_600,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Range::Hour => "1h",
            Range::SixHours => "6h",
            Range::Day => "24h",
            Range::Week => "7d",
            Range::Month => "30d",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|range| range.as_str() == value)
    }
}

/// One bucket of a metric series.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Bucket start, Unix seconds.
    pub t: i64,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub metric: String,
    pub range: Range,
    pub bucket_s: i64,
    pub from: i64,
    pub to: i64,
    pub points: Vec<Point>,
}

/// 24 h trend of every metric of every device, for the overview cards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sparkline {
    pub device_id: String,
    pub metric: String,
    pub bucket_s: i64,
    pub points: Vec<(i64, f64)>,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DevicePatch {
    pub name: Option<String>,
    pub location: Option<String>,
    pub alert_rules: Option<Vec<AlertRule>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeployRequest {
    /// Devices to update; all devices of the release's project when absent.
    pub devices: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeployReply {
    pub targeted: Vec<String>,
    pub already_current: Vec<String>,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    pub device: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub auth_required: bool,
    pub signed_in: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_round_trip() {
        for range in Range::ALL {
            assert_eq!(Range::parse(range.as_str()), Some(range));
            assert!(range.seconds() / range.bucket_s() <= 200);
        }
    }
}
