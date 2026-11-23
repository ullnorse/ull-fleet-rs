//! One device: live readings with charts, firmware and updates, alert
//! limits, details and activity.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use ull_fleet_types::{
    AlertRule, DeployReply, DevicePatch, DeviceView, EventView, Range, ReleaseView, Series,
    UpdateState, units,
};

use crate::api;
use crate::components::chart::LineChart;
use crate::components::events::EventList;
use crate::components::shell::PageHeader;
use crate::components::widgets::{ConfirmButton, Row, Segmented, Status};
use crate::format;
use crate::icons::{Icon, SignalBars, Svg};
use crate::pages::overview::flash_on_change;
use crate::store::{Tone, Watch, use_fleet};

#[component]
pub fn DevicePage() -> impl IntoView {
    let fleet = use_fleet();
    let params = use_params_map();
    let id = Memo::new(move |_| params.with(|p| p.get("id").unwrap_or_default()));
    let device = Memo::new(move |_| fleet.device(&id.get()).map(RwSignal::from));

    // Fast check-ins while this page is open and visible.
    let watch = StoredValue::new_local(None::<Watch>);
    Effect::new(move |_| {
        let id = id.get();
        watch.set_value(Some(Watch::open(&id)));
    });
    on_cleanup(move || watch.set_value(None));

    let title = Signal::derive(move || match device.get() {
        Some(d) => d.with(|d| d.display_name().to_string()),
        None => String::new(),
    });

    view! {
        <PageHeader title=title back="/" />
        <div class="page">
            {move || match device.get() {
                Some(device) => view! { <DeviceDetail device=device /> }.into_any(),
                None if fleet.loaded.get() => view! {
                    <div class="empty-state">
                        <h2>"Device not found"</h2>
                        <p>"It was removed, or has not checked in yet."</p>
                    </div>
                }
                .into_any(),
                None => view! { <div class="skeleton-grid"><div></div></div> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn DeviceDetail(device: RwSignal<DeviceView>) -> impl IntoView {
    let id = device.with_untracked(|d| d.id.clone());
    view! {
        <Hero device=device />
        <div class="device-layout">
            <div class="device-main">
                <Readings device=device />
            </div>
            <aside class="device-side">
                <FirmwareCard device=device />
                <AlertsCard device=device />
                <DetailsCard device=device />
                <DeviceActivity id=id.clone() />
                <RemoveDevice id=id />
            </aside>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Header block: status, location, rename.

#[component]
fn Hero(device: RwSignal<DeviceView>) -> impl IntoView {
    let fleet = use_fleet();
    let editing = RwSignal::new(false);
    let status = Memo::new(move |_| device.with(Status::of));
    let fresh = flash_on_change(Signal::derive(move || device.with(|d| d.last_seen)));
    let line = move || {
        let now = fleet.now.get();
        device.with(|d| {
            if !d.online {
                format!("Offline for {}", format::duration(now - d.last_seen))
            } else {
                let next = d.last_seen + d.poll_interval_s as i64 - now;
                if next > 0 {
                    format!(
                        "Checked in {} · next in {next} s",
                        format::ago(d.last_seen, now)
                    )
                } else {
                    format!("Checked in {} · next due", format::ago(d.last_seen, now))
                }
            }
        })
    };

    view! {
        <section class="hero" data-status=move || status.get().key()>
            <Show
                when=move || editing.get()
                fallback=move || {
                    view! {
                        <div class="hero-row">
                            <div class="hero-text">
                                <div class="hero-status">
                                    <span class="status-dot" class:fresh=move || fresh.get()></span>
                                    <span class="status-label">{move || status.get().label()}</span>
                                    <Show when=move || device.with(|d| d.live && d.online)>
                                        {move || {
                                            let live_s = fleet.server.with(|s| s.as_ref().map_or(5, |s| s.live_poll_interval_s));
                                            let poll_s = device.with(|d| d.poll_interval_s);
                                            if poll_s <= live_s {
                                                view! {
                                                    <span class="live-chip" title="Checks in every few seconds while this page is open">
                                                        {format!("Live · every {poll_s} s")}
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <span class="live-chip pending" title="Switches to fast check-ins at its next check-in">
                                                        "Live from next check-in"
                                                    </span>
                                                }.into_any()
                                            }
                                        }}
                                    </Show>
                                </div>
                                <p class="hero-line">{line}</p>
                                <p class="hero-meta">
                                    {move || device.with(|d| {
                                        let mut parts = Vec::new();
                                        if !d.location.is_empty() {
                                            parts.push(d.location.clone());
                                        }
                                        parts.push(d.project.clone());
                                        parts.join(" · ")
                                    })}
                                </p>
                            </div>
                            <button class="btn ghost" on:click=move |_| editing.set(true)>
                                <Svg icon=Icon::Edit />
                                <span>"Edit"</span>
                            </button>
                        </div>
                    }
                }
            >
                <EditForm device=device editing=editing />
            </Show>
        </section>
    }
}

#[component]
fn EditForm(device: RwSignal<DeviceView>, editing: RwSignal<bool>) -> impl IntoView {
    let fleet = use_fleet();
    let (name, location, id) =
        device.with_untracked(|d| (d.name.clone(), d.location.clone(), d.id.clone()));
    let name = RwSignal::new(name);
    let location = RwSignal::new(location);
    let saving = RwSignal::new(false);
    let locations = Memo::new(move |_| {
        let mut all: Vec<String> = fleet
            .all_devices()
            .iter()
            .map(|d| d.with(|d| d.location.clone()))
            .filter(|l| !l.is_empty())
            .collect();
        all.sort();
        all.dedup();
        all
    });
    let save = move || {
        let id = id.clone();
        saving.set(true);
        leptos::task::spawn_local(async move {
            let patch = DevicePatch {
                name: Some(name.get_untracked()),
                location: Some(location.get_untracked()),
                alert_rules: None,
            };
            match api::send::<_, DeviceView>("PATCH", &format!("/api/v1/devices/{id}"), &patch)
                .await
            {
                Ok(view) => {
                    device.set(view);
                    editing.set(false);
                }
                Err(err) => fleet.error(&err),
            }
            saving.set(false);
        });
    };
    let save = StoredValue::new(save);
    view! {
        <form class="edit-form" on:submit=move |ev| { ev.prevent_default(); save.with_value(|s| s()); }>
            <label class="field">
                <span>"Name"</span>
                <input
                    type="text"
                    maxlength="40"
                    placeholder=move || device.with(|d| d.id.clone())
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span>"Location"</span>
                <input
                    type="text"
                    maxlength="40"
                    list="locations"
                    placeholder="Kitchen"
                    prop:value=move || location.get()
                    on:input=move |ev| location.set(event_target_value(&ev))
                />
                <datalist id="locations">
                    {move || locations.get().into_iter().map(|l| view! { <option value=l></option> }).collect_view()}
                </datalist>
            </label>
            <div class="form-actions">
                <button type="button" class="btn ghost" on:click=move |_| editing.set(false)>"Cancel"</button>
                <button type="submit" class="btn primary" disabled=move || saving.get()>"Save"</button>
            </div>
        </form>
    }
}

// ---------------------------------------------------------------------------
// Readings and charts.

const RANGE_KEY: &str = "range";

fn stored_range() -> Range {
    window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item(RANGE_KEY).ok().flatten())
        .and_then(|v| Range::parse(&v))
        .unwrap_or(Range::SixHours)
}

#[component]
fn Readings(device: RwSignal<DeviceView>) -> impl IntoView {
    let fleet = use_fleet();
    let id = device.with_untracked(|d| d.id.clone());
    let range = RwSignal::new(stored_range());
    Effect::new(move |_| {
        let value = range.get().as_str();
        if let Ok(Some(storage)) = window().local_storage() {
            let _ = storage.set_item(RANGE_KEY, value);
        }
    });

    let metrics = Memo::new(move |_| {
        device.with(|d| {
            d.readings
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        })
    });
    let series = RwSignal::new(HashMap::<String, Series>::new());
    let loading = RwSignal::new(false);
    let generation = StoredValue::new(0u32);
    let last_fetch = StoredValue::new((None::<Range>, 0i64));

    // Refetch on range change, new metrics, or new readings (throttled by range).
    let latest =
        Memo::new(move |_| device.with(|d| d.readings.iter().map(|r| r.ts).max().unwrap_or(0)));
    let id_for_fetch = id.clone();
    Effect::new(move |_| {
        let range = range.get();
        let metrics = metrics.get();
        let latest = latest.get();
        let (last_range, last_ts) = last_fetch.get_value();
        let min_gap = match range {
            Range::Hour | Range::SixHours => 0,
            Range::Day => 60,
            Range::Week | Range::Month => 600,
        };
        if last_range == Some(range) && latest - last_ts < min_gap && latest != 0 {
            return;
        }
        last_fetch.set_value((Some(range), latest));
        generation.update_value(|g| *g += 1);
        let mine = generation.get_value();
        if last_range != Some(range) {
            loading.set(true);
        }
        let id = id_for_fetch.clone();
        leptos::task::spawn_local(async move {
            let mut fetched = HashMap::new();
            for metric in metrics {
                let url = format!(
                    "/api/v1/devices/{id}/series?metric={metric}&range={}",
                    range.as_str()
                );
                if let Ok(s) = api::get::<Series>(&url).await {
                    fetched.insert(metric, s);
                }
            }
            if generation.try_get_value() == Some(mine) {
                series.set(fetched);
                loading.set(false);
            }
        });
    });

    let gap_s = Signal::derive(move || {
        fleet
            .server
            .with(|s| s.as_ref().map_or(90, |s| s.poll_interval_s as i64 * 3))
    });
    let csv = move || {
        format!(
            "/api/v1/devices/{id}/readings.csv?range={}",
            range.get().as_str()
        )
    };

    view! {
        <div class="toolbar">
            <Segmented
                label="Time range"
                value=range
                options=Range::ALL.iter().map(|r| (*r, r.as_str())).collect()
            />
            <a class="btn ghost small" href=csv download>
                <Svg icon=Icon::Download />
                <span>"CSV"</span>
            </a>
        </div>
        <Show when=move || metrics.with(Vec::is_empty)>
            <div class="card empty-card">"This device sends no readings, only check-ins."</div>
        </Show>
        <For each=move || metrics.get() key=|m| m.clone() let:metric>
            <MetricCard device=device metric=metric series=series loading=loading gap_s=gap_s range=range />
        </For>
    }
}

#[component]
fn MetricCard(
    device: RwSignal<DeviceView>,
    metric: String,
    series: RwSignal<HashMap<String, Series>>,
    loading: RwSignal<bool>,
    gap_s: Signal<i64>,
    range: RwSignal<Range>,
) -> impl IntoView {
    let name = StoredValue::new(metric.clone());
    let reading = Memo::new(move |_| {
        device.with(|d| name.with_value(|m| d.reading(m).map(|r| (r.value, r.ts))))
    });
    let alert = Memo::new(move |_| {
        device.with(|d| name.with_value(|m| d.alerts.iter().find(|a| a.metric == *m).cloned()))
    });
    let fresh = flash_on_change(Signal::derive(move || reading.get().map(|r| r.1)));
    let my_series = Signal::derive(move || series.with(|s| name.with_value(|m| s.get(m).cloned())));
    let low_high = Memo::new(move |_| {
        my_series.with(|s| {
            let s = s.as_ref()?;
            let lo = s.points.iter().map(|p| p.min).fold(f64::INFINITY, f64::min);
            let hi = s
                .points
                .iter()
                .map(|p| p.max)
                .fold(f64::NEG_INFINITY, f64::max);
            lo.is_finite().then_some((lo, hi))
        })
    });
    // Dew point next to humidity, when the device also reports temperature.
    let dew_point = Memo::new(move |_| {
        if units::kind(&name.get_value()) != "humidity" || !name.get_value().starts_with("humidity")
        {
            return None;
        }
        device.with(|d| {
            let t = d
                .readings
                .iter()
                .find(|r| units::kind(&r.name) == "temperature")?;
            let h = d.reading(&name.get_value())?;
            format::dew_point(t.value, h.value)
        })
    });
    let limits = Signal::derive(move || {
        device.with(|d| {
            name.with_value(|m| {
                d.alert_rules
                    .iter()
                    .filter(|r| r.metric == *m)
                    .flat_map(|r| [r.min, r.max])
                    .flatten()
                    .collect::<Vec<_>>()
            })
        })
    });
    let kind = units::kind(&metric);
    view! {
        <section class=format!("card metric-card kind-{kind}") class:alerting=move || alert.with(Option::is_some)>
            <header class="metric-head">
                <div class="metric-now">
                    <span class="metric-label">{units::label(&metric)}</span>
                    <span class="metric-value" class:fresh=move || fresh.get()>
                        <span class="num">
                            {move || name.with_value(|m| reading.get().map(|r| format::number(m, r.0)).unwrap_or_default())}
                        </span>
                        <span class="unit">{units::symbol(&metric)}</span>
                    </span>
                </div>
                <dl class="metric-stats">
                    {move || low_high.get().map(|(lo, hi)| name.with_value(|m| view! {
                        <div><dt>{format!("Low {}", range.get().as_str())}</dt><dd>{format::with_unit(m, lo)}</dd></div>
                        <div><dt>{format!("High {}", range.get().as_str())}</dt><dd>{format::with_unit(m, hi)}</dd></div>
                    }))}
                    {move || dew_point.get().map(|dp| view! {
                        <div><dt>"Dew point"</dt><dd>{format::with_unit("dew_point_c", dp)}</dd></div>
                    })}
                </dl>
            </header>
            {move || alert.get().map(|a| name.with_value(|m| view! {
                <p class="metric-alert">
                    <Svg icon=Icon::Alert />
                    {format!("{} {} since {}", if a.direction == "above" { "Above" } else { "Below" }, format::with_unit(m, a.limit), format::clock(a.since))}
                </p>
            }))}
            <LineChart series=my_series metric=metric.clone() loading=loading gap_s=gap_s limits=limits />
        </section>
    }
}

// ---------------------------------------------------------------------------
// Firmware and updates.

#[component]
fn FirmwareCard(device: RwSignal<DeviceView>) -> impl IntoView {
    let fleet = use_fleet();
    let id = device.with_untracked(|d| d.id.clone());
    let candidates = Memo::new(move |_| {
        let (project, version, secure) =
            device.with(|d| (d.project.clone(), d.version.clone(), d.secure_version));
        fleet.releases.with(|releases| {
            releases
                .iter()
                .filter(|r| {
                    r.project == project && r.version != version && r.secure_version >= secure
                })
                .cloned()
                .collect::<Vec<ReleaseView>>()
        })
    });
    let chosen = RwSignal::new(None::<i64>);
    let busy = RwSignal::new(false);
    let id_install = id.clone();
    let install = move |release_id: i64| {
        let id = id_install.clone();
        busy.set(true);
        leptos::task::spawn_local(async move {
            #[derive(serde::Serialize)]
            struct Body {
                release_id: i64,
            }
            let result = api::send::<_, DeployReply>(
                "POST",
                &format!("/api/v1/devices/{id}/update"),
                &Body { release_id },
            )
            .await;
            if let Err(err) = result {
                fleet.error(&err);
            }
            busy.set(false);
        });
    };
    let install = StoredValue::new(install);
    let cancel = move || {
        let id = id.clone();
        leptos::task::spawn_local(async move {
            if let Err(err) =
                api::send_empty::<()>("DELETE", &format!("/api/v1/devices/{id}/update"), None).await
            {
                fleet.error(&err);
            }
        });
    };
    let cancel = StoredValue::new(cancel);

    let running = move || {
        device.with(|d| {
            let v = format::version(&d.version);
            (v.base.to_string(), v.rev.map(str::to_string), v.built)
        })
    };

    view! {
        <section class="card">
            <h2 class="card-title"><Svg icon=Icon::Cpu />"Firmware"</h2>
            <div class="firmware-now">
                <span class="version-big">{move || running().0}</span>
                {move || running().1.map(|rev| view! { <code class="rev">{rev}</code> })}
                {move || running().2.map(|built| view! { <span class="muted">{format!("built {}", format::day_clock(built))}</span> })}
            </div>
            {move || {
                let (state, target, attempts, max, progress, error, online) = device.with(|d| (
                    d.update.state,
                    d.update.target_version.clone(),
                    d.update.attempts,
                    d.update.max_attempts,
                    d.update.progress,
                    d.last_ota_error.clone(),
                    d.online,
                ));
                let target_short = target.as_deref().map(format::short_version).unwrap_or_default();
                match state {
                    UpdateState::Pending | UpdateState::Downloading => {
                        let step = match (state, progress) {
                            (UpdateState::Pending, _) => 0,
                            (_, Some(p)) if !p.done => 1,
                            _ => 2,
                        };
                        let pct = progress.map(|p| p.sent as f64 / p.total.max(1) as f64 * 100.0);
                        view! {
                            <div class="update-box">
                                <p class="update-title">{format!("Updating to {target_short}")}</p>
                                <ol class="stepper">
                                    {["Queued", "Download", "Install", "Confirmed"].into_iter().enumerate().map(|(i, label)| {
                                        let class = if i < step { "done" } else if i == step { "current" } else { "" };
                                        view! { <li class=class><span class="step-dot"></span><span>{label}</span></li> }
                                    }).collect_view()}
                                </ol>
                                {match step {
                                    0 => view! { <p class="muted">{if online { "Starts at the next check-in." } else { "Starts when the device is back online." }}</p> }.into_any(),
                                    1 => {
                                        let pct = pct.unwrap_or(0.0);
                                        let p = progress.unwrap_or(ull_fleet_types::Progress { sent: 0, total: 0, done: false });
                                        view! {
                                            <div class="progress">
                                                <div class="bar big"><div style=format!("width:{pct:.1}%")></div></div>
                                                <span class="num">{format!("{pct:.0} % · {} of {}", format::bytes(p.sent as i64), format::bytes(p.total as i64))}</span>
                                            </div>
                                        }.into_any()
                                    }
                                    _ => view! { <p class="muted">"Verifying the image and restarting. The device confirms the update on its first check-in."</p> }.into_any(),
                                }}
                                {(attempts > 0).then(|| view! { <p class="muted">{format!("Attempt {} of {max}", attempts + 1)}</p> })}
                                <button class="btn ghost small" on:click=move |_| cancel.with_value(|c| c())>"Cancel update"</button>
                            </div>
                        }.into_any()
                    }
                    UpdateState::Failed => {
                        let release_id = device.with(|d| d.update.release_id);
                        view! {
                            <div class="update-box failed">
                                <p class="update-title"><Svg icon=Icon::Alert />{format!("Update to {target_short} failed {attempts} times")}</p>
                                {error.map(|e| view! { <p class="muted">{e}</p> })}
                                <div class="form-actions">
                                    <button class="btn ghost small" on:click=move |_| cancel.with_value(|c| c())>"Dismiss"</button>
                                    {release_id.map(|rid| view! {
                                        <button class="btn primary small" disabled=move || busy.get() on:click=move |_| install.with_value(|f| f(rid))>"Try again"</button>
                                    })}
                                </div>
                            </div>
                        }.into_any()
                    }
                    UpdateState::None => view! {
                        <Show
                            when=move || candidates.with(|c| !c.is_empty())
                            fallback=|| view! { <p class="muted up-to-date"><Svg icon=Icon::Check />"No newer release uploaded"</p> }
                        >
                            <div class="install">
                                <select
                                    aria-label="Release"
                                    on:change=move |ev| chosen.set(event_target_value(&ev).parse().ok())
                                >
                                    {move || candidates.get().into_iter().map(|r| {
                                        let label = match format::version(&r.version).built {
                                            Some(b) => format!("{} · {}", format::short_version(&r.version), format::day_clock(b)),
                                            None => format::short_version(&r.version),
                                        };
                                        view! { <option value=r.id.to_string()>{label}</option> }
                                    }).collect_view()}
                                </select>
                                <button
                                    class="btn primary"
                                    disabled=move || busy.get()
                                    on:click=move |_| {
                                        let id = chosen.get_untracked().or_else(|| candidates.with_untracked(|c| c.first().map(|r| r.id)));
                                        if let Some(id) = id { install.with_value(|f| f(id)); }
                                    }
                                >
                                    <Svg icon=Icon::Download />
                                    <span>"Install"</span>
                                </button>
                            </div>
                        </Show>
                    }.into_any(),
                }
            }}
        </section>
    }
}

// ---------------------------------------------------------------------------
// Alert limits.

#[derive(Clone, PartialEq)]
struct Draft {
    metric: String,
    min: String,
    max: String,
}

fn drafts(d: &DeviceView) -> Vec<Draft> {
    d.readings
        .iter()
        .map(|r| {
            let rule = d.alert_rules.iter().find(|rule| rule.metric == r.name);
            let show = |v: Option<f64>| v.map(|v| format::number(&r.name, v)).unwrap_or_default();
            Draft {
                metric: r.name.clone(),
                min: show(rule.and_then(|r| r.min)),
                max: show(rule.and_then(|r| r.max)),
            }
        })
        .collect()
}

#[component]
fn AlertsCard(device: RwSignal<DeviceView>) -> impl IntoView {
    let fleet = use_fleet();
    let saved = Memo::new(move |_| device.with(drafts));
    let draft = RwSignal::new(saved.get_untracked());
    let dirty = Memo::new(move |_| draft.get() != saved.get());
    // Follow server changes while nothing is being edited.
    Effect::new(move |_| {
        let saved = saved.get();
        if !dirty.get_untracked() {
            draft.set(saved);
        }
    });
    let saving = RwSignal::new(false);
    let save = move || {
        let id = device.with_untracked(|d| d.id.clone());
        let parse = |text: &str| -> Result<Option<f64>, ()> {
            let t = text.trim().replace(',', ".");
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse().map(Some).map_err(|_| ())
            }
        };
        let mut rules = Vec::new();
        for d in draft.get_untracked() {
            let (Ok(min), Ok(max)) = (parse(&d.min), parse(&d.max)) else {
                fleet.toast(
                    Tone::Critical,
                    "Not saved".into(),
                    format!("{}: limits must be numbers", units::label(&d.metric)),
                    None,
                );
                return;
            };
            if min.is_some() || max.is_some() {
                rules.push(AlertRule {
                    metric: d.metric,
                    min,
                    max,
                });
            }
        }
        saving.set(true);
        leptos::task::spawn_local(async move {
            let patch = DevicePatch {
                name: None,
                location: None,
                alert_rules: Some(rules),
            };
            match api::send::<_, DeviceView>("PATCH", &format!("/api/v1/devices/{id}"), &patch)
                .await
            {
                Ok(view) => {
                    draft.set(drafts(&view));
                    device.set(view);
                }
                Err(err) => fleet.error(&err),
            }
            saving.set(false);
        });
    };
    let save = StoredValue::new(save);
    view! {
        <section class="card">
            <h2 class="card-title"><Svg icon=Icon::Bell />"Alert limits"</h2>
            <Show
                when=move || draft.with(|d| !d.is_empty())
                fallback=|| view! { <p class="muted">"No readings to set limits on."</p> }
            >
                <div class="limits">
                    <div class="limits-head"><span></span><span>"Below"</span><span>"Above"</span></div>
                    <For
                        each={move || draft.get().into_iter().enumerate().map(|(i, d)| (i, d.metric)).collect::<Vec<_>>()}
                        key=|(_, m)| m.clone()
                        let:item
                    >
                        {
                            let (index, metric) = item;
                            let symbol = units::symbol(&metric);
                            let active = Memo::new({
                                let metric = metric.clone();
                                move |_| device.with(|d| d.alerts.iter().any(|a| a.metric == metric))
                            });
                            view! {
                                <div class="limit-row" class:alerting=move || active.get()>
                                    <span class="limit-label">{units::label(&metric)}</span>
                                    <label class="limit-input">
                                        <input
                                            type="text"
                                            inputmode="decimal"
                                            placeholder="–"
                                            aria-label=format!("{} below", units::label(&metric))
                                            prop:value=move || draft.with(|d| d.get(index).map(|d| d.min.clone()).unwrap_or_default())
                                            on:input=move |ev| draft.update(|d| if let Some(d) = d.get_mut(index) { d.min = event_target_value(&ev) })
                                        />
                                        <span>{symbol}</span>
                                    </label>
                                    <label class="limit-input">
                                        <input
                                            type="text"
                                            inputmode="decimal"
                                            placeholder="–"
                                            aria-label=format!("{} above", units::label(&metric))
                                            prop:value=move || draft.with(|d| d.get(index).map(|d| d.max.clone()).unwrap_or_default())
                                            on:input=move |ev| draft.update(|d| if let Some(d) = d.get_mut(index) { d.max = event_target_value(&ev) })
                                        />
                                        <span>{symbol}</span>
                                    </label>
                                </div>
                            }
                        }
                    </For>
                </div>
                <div class="form-actions" class:hidden=move || !dirty.get()>
                    <button class="btn ghost small" on:click=move |_| draft.set(saved.get_untracked())>"Reset"</button>
                    <button class="btn primary small" disabled=move || saving.get() on:click=move |_| save.with_value(|s| s())>"Save limits"</button>
                </div>
            </Show>
        </section>
    }
}

// ---------------------------------------------------------------------------
// Details, activity, removal.

fn mac(id: &str) -> String {
    id.as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

#[component]
fn DetailsCard(device: RwSignal<DeviceView>) -> impl IntoView {
    let fleet = use_fleet();
    let uptime = move || {
        let now = fleet.now.get();
        device.with(|d| {
            if d.online {
                format::duration(d.uptime_s + (now - d.last_seen).max(0))
            } else {
                "–".to_string()
            }
        })
    };
    view! {
        <section class="card">
            <h2 class="card-title"><Svg icon=Icon::Devices />"Details"</h2>
            <dl class="rows">
                <Row label="Signal">
                    <span class="signal-row">
                        <SignalBars bars=Signal::derive(move || device.with(|d| format::signal_bars(d.rssi_dbm))) />
                        {move || device.with(|d| match d.rssi_dbm {
                            Some(r) => format!("{} · {r} dBm", format::signal_label(Some(r))),
                            None => "Unknown".into(),
                        })}
                    </span>
                </Row>
                <Row label="Uptime"><span class="num">{uptime}</span></Row>
                <Row label="IP address">{move || device.with(|d| if d.ip.is_empty() { "–".into() } else { d.ip.clone() })}</Row>
                <Row label="MAC address"><code>{move || device.with(|d| mac(&d.id))}</code></Row>
                <Row label="App slot">{move || device.with(|d| format!("{} · {}", d.partition, d.ota_state))}</Row>
                <Row label="Check-in">{move || device.with(|d| format!("every {} s", d.poll_interval_s))}</Row>
                <Row label="First seen">{move || device.with(|d| format::day_clock(d.first_seen))}</Row>
            </dl>
        </section>
    }
}

#[component]
fn DeviceActivity(id: String) -> impl IntoView {
    let fleet = use_fleet();
    let older = RwSignal::new(Vec::<EventView>::new());
    let exhausted = RwSignal::new(false);
    let id = StoredValue::new(id);
    let fetch = move |before: Option<i64>| {
        let id = id.get_value();
        leptos::task::spawn_local(async move {
            let mut url = format!("/api/v1/events?device={id}&limit=30");
            if let Some(before) = before {
                url.push_str(&format!("&before={before}"));
            }
            if let Ok(events) = api::get::<Vec<EventView>>(&url).await {
                exhausted.set(events.len() < 30);
                older.update(|o| o.extend(events));
            }
        });
    };
    fetch(None);
    let events = Signal::derive(move || {
        let id = id.get_value();
        let mut all: Vec<EventView> = fleet
            .events
            .with(|e| e.iter().filter(|e| e.device_id == id).cloned().collect());
        older.with(|o| all.extend(o.iter().cloned()));
        all.sort_by_key(|e| std::cmp::Reverse(e.id));
        all.dedup_by_key(|e| e.id);
        all
    });
    view! {
        <section class="card">
            <h2 class="card-title"><Svg icon=Icon::Activity />"Activity"</h2>
            <EventList events=events />
            <Show when=move || !exhausted.get()>
                <button
                    class="btn ghost small wide"
                    on:click=move |_| fetch(older.with_untracked(|o| o.last().map(|e| e.id)))
                >
                    "Older"
                </button>
            </Show>
        </section>
    }
}

#[component]
fn RemoveDevice(id: String) -> impl IntoView {
    let fleet = use_fleet();
    let navigate = use_navigate();
    view! {
        <div class="danger">
            <ConfirmButton
                label="Remove device"
                confirm="Tap again to remove"
                class="danger-btn"
                icon=Icon::Trash
                on_confirm=move || {
                    let id = id.clone();
                    let navigate = navigate.clone();
                    leptos::task::spawn_local(async move {
                        match api::send_empty::<()>("DELETE", &format!("/api/v1/devices/{id}"), None).await {
                            Ok(()) => navigate("/", Default::default()),
                            Err(err) => fleet.error(&err),
                        }
                    });
                }
            />
            <p class="muted">"Deletes its readings and history. It registers again on its next check-in."</p>
        </div>
    }
}
