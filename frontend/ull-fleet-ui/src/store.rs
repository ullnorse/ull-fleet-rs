//! Client-side fleet state, kept current by the server's event stream.

use std::cell::RefCell;
use std::collections::HashMap;

use leptos::prelude::*;
use ull_fleet_types::{
    DeviceView, EventView, Progress, ReleaseView, ServerInfo, SessionInfo, Snapshot, Sparkline,
    StreamMessage,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{EventSource, MessageEvent};

use crate::api;
use crate::format::browser_now;

/// Events kept in memory for the activity views.
const MAX_EVENTS: usize = 400;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Live,
    Reconnecting,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    Unknown,
    SignInNeeded,
    SignedIn,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warning,
    Critical,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub tone: Tone,
    pub title: String,
    pub body: String,
    pub device_id: Option<String>,
}

/// 30-minute buckets: `(bucket start, sum, count)`.
pub type SparkPoints = Vec<(i64, f64, u32)>;

#[derive(Copy, Clone)]
pub struct Fleet {
    pub server: RwSignal<Option<ServerInfo>>,
    /// Device ids in display order: by location, then name.
    pub ids: RwSignal<Vec<String>>,
    devices: StoredValue<HashMap<String, ArcRwSignal<DeviceView>>>,
    pub releases: RwSignal<Vec<ReleaseView>>,
    /// Newest first.
    pub events: RwSignal<Vec<EventView>>,
    pub sparklines: RwSignal<HashMap<(String, String), SparkPoints>>,
    pub connection: RwSignal<Connection>,
    pub auth: RwSignal<Auth>,
    pub loaded: RwSignal<bool>,
    /// Server clock, ticking every second.
    pub now: RwSignal<i64>,
    clock_offset: StoredValue<f64>,
    pub toasts: RwSignal<Vec<Toast>>,
    next_toast: StoredValue<u64>,
}

pub fn use_fleet() -> Fleet {
    expect_context::<Fleet>()
}

thread_local! {
    static STREAM: RefCell<Option<Stream>> = const { RefCell::new(None) };
}

struct Stream {
    source: EventSource,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_open: Closure<dyn FnMut(web_sys::Event)>,
    _on_error: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.source.close();
    }
}

impl Fleet {
    pub fn new() -> Self {
        Self {
            server: RwSignal::new(None),
            ids: RwSignal::new(Vec::new()),
            devices: StoredValue::new(HashMap::new()),
            releases: RwSignal::new(Vec::new()),
            events: RwSignal::new(Vec::new()),
            sparklines: RwSignal::new(HashMap::new()),
            connection: RwSignal::new(Connection::Connecting),
            auth: RwSignal::new(Auth::Unknown),
            loaded: RwSignal::new(false),
            now: RwSignal::new(browser_now() as i64),
            clock_offset: StoredValue::new(0.0),
            toasts: RwSignal::new(Vec::new()),
            next_toast: StoredValue::new(1),
        }
    }

    /// The device's signal. Reactive on the device list, so a page opened
    /// before the first snapshot picks the device up when it arrives.
    pub fn device(&self, id: &str) -> Option<ArcRwSignal<DeviceView>> {
        self.ids.track();
        self.devices.with_value(|devices| devices.get(id).cloned())
    }

    /// The device's signal, without subscribing to the device list.
    pub fn device_untracked(&self, id: &str) -> Option<ArcRwSignal<DeviceView>> {
        self.devices.with_value(|devices| devices.get(id).cloned())
    }

    pub fn device_name(&self, id: &str) -> String {
        self.devices.with_value(|devices| {
            devices
                .get(id)
                .map(|device| device.with_untracked(|d| d.display_name().to_string()))
                .unwrap_or_else(|| id.to_string())
        })
    }

    pub fn all_devices(&self) -> Vec<ArcRwSignal<DeviceView>> {
        let ids = self.ids.get();
        self.devices.with_value(|devices| {
            ids.iter()
                .filter_map(|id| devices.get(id).cloned())
                .collect()
        })
    }

    fn resort(&self) {
        let mut entries: Vec<(String, String, String)> = self.devices.with_value(|devices| {
            devices
                .values()
                .map(|device| {
                    device.with_untracked(|d| {
                        (
                            d.location.to_lowercase(),
                            d.display_name().to_lowercase(),
                            d.id.clone(),
                        )
                    })
                })
                .collect()
        });
        // Devices without a location go last.
        entries.sort_by(|a, b| {
            (a.0.is_empty(), &a.0, &a.1, &a.2).cmp(&(b.0.is_empty(), &b.0, &b.1, &b.2))
        });
        let ids: Vec<String> = entries.into_iter().map(|(_, _, id)| id).collect();
        if self.ids.with_untracked(|current| *current != ids) {
            self.ids.set(ids);
        }
    }

    fn apply_snapshot(&self, snapshot: Snapshot) {
        self.clock_offset
            .set_value(snapshot.server.now as f64 - browser_now());
        self.tick();
        self.server.set(Some(snapshot.server));
        let mut seen = Vec::new();
        for device in snapshot.devices {
            seen.push(device.id.clone());
            self.upsert(device);
        }
        self.devices
            .update_value(|devices| devices.retain(|id, _| seen.contains(id)));
        self.resort();
        self.releases.set(snapshot.releases);
        self.events.set(snapshot.events);
        self.loaded.set(true);
    }

    /// Updates a device; returns whether the device list changed shape.
    fn upsert(&self, device: DeviceView) -> bool {
        let existing = self
            .devices
            .with_value(|devices| devices.get(&device.id).cloned());
        match existing {
            Some(signal) => {
                let reorder = signal.with_untracked(|old| {
                    old.name != device.name || old.location != device.location
                });
                let fresh: Vec<(String, f64, i64)> = signal.with_untracked(|old| {
                    device
                        .readings
                        .iter()
                        .filter(|r| old.reading(&r.name).is_none_or(|o| o.ts != r.ts))
                        .map(|r| (r.name.clone(), r.value, r.ts))
                        .collect()
                });
                if !fresh.is_empty() {
                    self.extend_sparklines(&device.id, &fresh);
                }
                if signal.with_untracked(|old| *old != device) {
                    signal.set(device);
                }
                reorder
            }
            None => {
                let id = device.id.clone();
                self.devices.update_value(|devices| {
                    devices.insert(id, ArcRwSignal::new(device));
                });
                true
            }
        }
    }

    fn extend_sparklines(&self, device_id: &str, readings: &[(String, f64, i64)]) {
        self.sparklines.update(|lines| {
            for (metric, value, ts) in readings {
                let bucket = ts / 1800 * 1800;
                let points = lines
                    .entry((device_id.to_string(), metric.clone()))
                    .or_default();
                match points.last_mut() {
                    Some(last) if last.0 == bucket => {
                        last.1 += value;
                        last.2 += 1;
                    }
                    _ => points.push((bucket, *value, 1)),
                }
                let cutoff = ts - 86_400;
                points.retain(|point| point.0 >= cutoff);
            }
        });
    }

    fn set_progress(&self, device_id: &str, progress: Progress) {
        if let Some(signal) = self
            .devices
            .with_value(|devices| devices.get(device_id).cloned())
        {
            signal.update(|device| device.update.progress = Some(progress));
        }
    }

    fn apply(&self, message: StreamMessage) {
        match message {
            StreamMessage::Snapshot(snapshot) => self.apply_snapshot(*snapshot),
            StreamMessage::Device(device) => {
                if self.upsert(device) {
                    self.resort();
                }
            }
            StreamMessage::DeviceRemoved { id } => {
                self.devices.update_value(|devices| {
                    devices.remove(&id);
                });
                self.resort();
            }
            StreamMessage::Event(event) => {
                self.toast_for(&event);
                self.events.update(|events| {
                    if events.iter().any(|e| e.id == event.id) {
                        return;
                    }
                    events.insert(0, event);
                    events.truncate(MAX_EVENTS);
                });
            }
            StreamMessage::Progress {
                device_id,
                progress,
            } => self.set_progress(&device_id, progress),
            StreamMessage::Releases { releases } => self.releases.set(releases),
        }
    }

    fn toast_for(&self, event: &EventView) {
        let tone = match event.kind.as_str() {
            "alert" | "update_failed" | "crashed" => Tone::Critical,
            "offline" => Tone::Warning,
            "update" | "alert_cleared" | "online" => Tone::Good,
            "registered" => Tone::Info,
            _ => return,
        };
        self.toast(
            tone,
            self.device_name(&event.device_id),
            crate::format::tidy_message(&event.message),
            Some(event.device_id.clone()),
        );
    }

    pub fn toast(&self, tone: Tone, title: String, body: String, device_id: Option<String>) {
        let id = self.next_toast.get_value();
        self.next_toast.set_value(id + 1);
        self.toasts.update(|toasts| {
            toasts.push(Toast {
                id,
                tone,
                title,
                body,
                device_id,
            });
            if toasts.len() > 3 {
                toasts.remove(0);
            }
        });
        let toasts = self.toasts;
        set_timeout(
            move || toasts.update(|list| list.retain(|toast| toast.id != id)),
            std::time::Duration::from_secs(6),
        );
    }

    pub fn error(&self, err: &api::Error) {
        if *err == api::Error::Unauthorized {
            self.auth.set(Auth::SignInNeeded);
            return;
        }
        self.toast(Tone::Critical, "Not done".into(), err.to_string(), None);
    }

    fn tick(&self) {
        let now = (browser_now() + self.clock_offset.get_value()).floor() as i64;
        if self.now.get_untracked() != now {
            self.now.set(now);
        }
    }

    pub fn start(self) {
        let clock = self;
        set_interval(move || clock.tick(), std::time::Duration::from_millis(1000));

        let on_visible = Closure::<dyn FnMut()>::new(move || {
            if is_visible() {
                self.tick();
                let closed = STREAM.with(|stream| {
                    stream
                        .borrow()
                        .as_ref()
                        .is_none_or(|s| s.source.ready_state() == EventSource::CLOSED)
                });
                if closed && self.auth.get_untracked() != Auth::SignInNeeded {
                    self.connect();
                }
                self.load_sparklines();
            }
        });
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            let _ = document.add_event_listener_with_callback(
                "visibilitychange",
                on_visible.as_ref().unchecked_ref(),
            );
        }
        on_visible.forget();

        leptos::task::spawn_local(async move {
            self.check_session().await;
        });
    }

    pub async fn check_session(self) {
        match api::get::<SessionInfo>("/api/v1/session").await {
            Ok(info) if info.signed_in => {
                self.auth.set(Auth::SignedIn);
                self.connect();
                self.load_sparklines();
            }
            Ok(_) => {
                self.disconnect();
                self.auth.set(Auth::SignInNeeded);
            }
            Err(_) => {
                self.connection.set(Connection::Reconnecting);
                set_timeout(
                    move || leptos::task::spawn_local(self.check_session()),
                    std::time::Duration::from_secs(3),
                );
            }
        }
    }

    pub fn load_sparklines(self) {
        leptos::task::spawn_local(async move {
            if let Ok(lines) = api::get::<Vec<Sparkline>>("/api/v1/sparklines").await {
                let map = lines
                    .into_iter()
                    .map(|line| {
                        (
                            (line.device_id, line.metric),
                            line.points.into_iter().map(|(t, v)| (t, v, 1)).collect(),
                        )
                    })
                    .collect();
                self.sparklines.set(map);
            }
        });
    }

    pub fn disconnect(&self) {
        STREAM.with(|stream| stream.borrow_mut().take());
    }

    pub fn connect(self) {
        self.disconnect();
        let Ok(source) = EventSource::new("/api/v1/stream") else {
            return;
        };
        if self.connection.get_untracked() != Connection::Connecting {
            self.connection.set(Connection::Reconnecting);
        }
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let Some(text) = event.data().as_string() else {
                return;
            };
            match serde_json::from_str::<StreamMessage>(&text) {
                Ok(message) => self.apply(message),
                Err(err) => leptos::logging::warn!("bad stream message: {err}"),
            }
        });
        let on_open = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            self.connection.set(Connection::Live);
        });
        let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            self.connection.set(Connection::Reconnecting);
            let closed = STREAM.with(|stream| {
                stream
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.source.ready_state() == EventSource::CLOSED)
            });
            if closed {
                // The browser gave up (server down or 401): find out which.
                self.disconnect();
                set_timeout(
                    move || leptos::task::spawn_local(self.check_session()),
                    std::time::Duration::from_secs(2),
                );
            }
        });
        source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        STREAM.with(|stream| {
            *stream.borrow_mut() = Some(Stream {
                source,
                _on_message: on_message,
                _on_open: on_open,
                _on_error: on_error,
            })
        });
    }
}

pub fn is_visible() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_none_or(|d| d.visibility_state() == web_sys::VisibilityState::Visible)
}

/// Keeps `/api/v1/devices/{id}/watch` open while the returned guard lives, so
/// the device checks in at the live interval. Reopens after the page was
/// hidden.
pub struct Watch {
    source: RefCell<Option<EventSource>>,
}

impl Watch {
    pub fn open(id: &str) -> Self {
        let url = format!("/api/v1/devices/{id}/watch");
        Self {
            source: RefCell::new(EventSource::new(&url).ok()),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(source) = self.source.borrow_mut().take() {
            source.close();
        }
    }
}
