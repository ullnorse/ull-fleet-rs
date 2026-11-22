//! Live updates: a broadcast of [`StreamMessage`]s for the UI's event stream,
//! plus the in-memory state behind them (who is watching which device,
//! firmware download progress, last known online state).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use ull_fleet_types::{Progress, StreamMessage};

use crate::AppState;
use crate::db::now;
use crate::error::AppError;
use crate::{fleet, views};

/// Messages a slow client may fall behind by before it is disconnected
/// (it reconnects and gets a fresh snapshot).
const BACKLOG: usize = 256;

pub struct Live {
    tx: broadcast::Sender<Arc<str>>,
    watchers: Mutex<HashMap<String, usize>>,
    progress: Mutex<HashMap<String, Progress>>,
    online: Mutex<HashMap<String, bool>>,
    /// Highest event id already broadcast; the lock also orders publishing.
    last_event_id: tokio::sync::Mutex<Option<i64>>,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(BACKLOG).0,
            watchers: Mutex::default(),
            progress: Mutex::default(),
            online: Mutex::default(),
            last_event_id: tokio::sync::Mutex::new(None),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Live {
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.tx.subscribe()
    }

    pub fn send(&self, message: &StreamMessage) {
        if self.tx.receiver_count() == 0 {
            return;
        }
        match serde_json::to_string(message) {
            Ok(json) => {
                let _ = self.tx.send(json.into());
            }
            Err(err) => tracing::warn!(error = %err, "failed to serialize stream message"),
        }
    }

    pub fn is_watched(&self, device_id: &str) -> bool {
        lock(&self.watchers).get(device_id).is_some_and(|n| *n > 0)
    }

    pub fn progress(&self, device_id: &str) -> Option<Progress> {
        lock(&self.progress).get(device_id).copied()
    }

    pub fn set_progress(&self, device_id: &str, progress: Progress) {
        lock(&self.progress).insert(device_id.to_string(), progress);
        self.send(&StreamMessage::Progress {
            device_id: device_id.to_string(),
            progress,
        });
    }

    pub fn clear_progress(&self, device_id: &str) {
        lock(&self.progress).remove(device_id);
    }
}

/// Keeps a device in fast check-in mode while a device page is open.
pub struct WatchGuard {
    state: AppState,
    device_id: String,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let became_unwatched = {
            let mut watchers = lock(&self.state.live.watchers);
            match watchers.get_mut(&self.device_id) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => {
                    watchers.remove(&self.device_id);
                    true
                }
                None => false,
            }
        };
        if became_unwatched && let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = self.state.clone();
            let id = std::mem::take(&mut self.device_id);
            handle.spawn(async move { state.publish_device(&id).await });
        }
    }
}

impl AppState {
    /// Check-in interval to give `device_id` now.
    pub fn poll_interval_for(&self, device_id: &str) -> u64 {
        if self.live.is_watched(device_id) {
            self.config.live_poll_interval.as_secs()
        } else {
            self.config.poll_interval.as_secs()
        }
    }

    pub fn watch(&self, device_id: &str) -> WatchGuard {
        let first = {
            let mut watchers = lock(&self.live.watchers);
            let count = watchers.entry(device_id.to_string()).or_default();
            *count += 1;
            *count == 1
        };
        if first {
            let state = self.clone();
            let id = device_id.to_string();
            tokio::spawn(async move { state.publish_device(&id).await });
        }
        WatchGuard {
            state: self.clone(),
            device_id: device_id.to_string(),
        }
    }

    /// Broadcasts the current view of a device, or its removal.
    pub async fn publish_device(&self, device_id: &str) {
        let state = self.clone();
        let id = device_id.to_string();
        let result = self
            .db
            .call(move |conn| views::load_device(conn, &id, &state.live, now()))
            .await;
        match result {
            Ok(view) => {
                lock(&self.live.online).insert(view.id.clone(), view.online);
                self.live.send(&StreamMessage::Device(view));
            }
            Err(AppError::NotFound) => {
                lock(&self.live.online).remove(device_id);
                self.live.clear_progress(device_id);
                self.live.send(&StreamMessage::DeviceRemoved {
                    id: device_id.to_string(),
                });
            }
            Err(err) => tracing::warn!(error = %err, "failed to publish device"),
        }
    }

    /// Broadcasts events added since the last call.
    pub async fn publish_events(&self) {
        let mut last = self.live.last_event_id.lock().await;
        let after = match *last {
            Some(id) => id,
            None => match self.db.call(|conn| fleet::last_event_id(conn)).await {
                Ok(id) => {
                    *last = Some(id);
                    return;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to read events");
                    return;
                }
            },
        };
        match self
            .db
            .call(move |conn| fleet::events_after(conn, after))
            .await
        {
            Ok(events) => {
                for event in events {
                    *last = Some(event.id);
                    self.live.send(&StreamMessage::Event(event));
                }
            }
            Err(err) => tracing::warn!(error = %err, "failed to publish events"),
        }
    }

    pub async fn publish_releases(&self) {
        match self.db.call(|conn| views::releases(conn)).await {
            Ok(releases) => self.live.send(&StreamMessage::Releases { releases }),
            Err(err) => tracing::warn!(error = %err, "failed to publish releases"),
        }
    }

    /// Publishes everything a change to these devices may have touched.
    pub async fn publish_changes(&self, device_ids: &[String], releases: bool) {
        for id in device_ids {
            self.publish_device(id).await;
        }
        self.publish_events().await;
        if releases {
            self.publish_releases().await;
        }
    }
}

/// Notices devices going offline (nothing else happens at that moment, so no
/// request would report it) and records it as an event.
pub fn spawn_offline_monitor(state: AppState) {
    tokio::spawn(async move {
        // Start from the current state without events.
        state.publish_events().await;
        if let Ok(devices) = state.db.call(|conn| fleet::list_devices(conn)).await {
            let time = now();
            let mut online = lock(&state.live.online);
            for device in devices {
                let is_online = device.is_online(time);
                online.insert(device.id, is_online);
            }
        }
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            let devices = match state.db.call(|conn| fleet::list_devices(conn)).await {
                Ok(devices) => devices,
                Err(err) => {
                    tracing::warn!(error = %err, "offline check failed");
                    continue;
                }
            };
            let time = now();
            let went_offline: Vec<(String, i64)> = {
                let mut online = lock(&state.live.online);
                devices
                    .iter()
                    .filter_map(|device| {
                        let is_online = device.is_online(time);
                        let was_online = online.insert(device.id.clone(), is_online);
                        (was_online == Some(true) && !is_online)
                            .then(|| (device.id.clone(), device.last_seen))
                    })
                    .collect()
            };
            for (id, last_seen) in went_offline {
                let event_id = id.clone();
                let message = format!(
                    "Offline: no check-in since {} ago",
                    fleet::duration(time - last_seen)
                );
                let _ = state
                    .db
                    .call(move |conn| fleet::add_event(conn, &event_id, time, "offline", &message))
                    .await;
                state.live.clear_progress(&id);
                state.publish_changes(&[id], false).await;
            }
        }
    });
}
