//! Devices: fleet summary and a live card per device, grouped by location.

use leptos::prelude::*;
use leptos_router::components::A;
use ull_fleet_types::{DeviceView, UpdateState, units};

use crate::components::chart::Sparkline;
use crate::components::shell::PageHeader;
use crate::components::widgets::Status;
use crate::format;
use crate::icons::{Icon, SignalBars, Svg};
use crate::store::use_fleet;

/// Readings shown on a card.
const CARD_READINGS: usize = 2;

#[component]
pub fn Overview() -> impl IntoView {
    let fleet = use_fleet();
    let counts = Memo::new(move |_| {
        let devices = fleet.all_devices();
        let mut online = 0;
        let mut alerts = 0;
        let mut updating = 0;
        for device in &devices {
            device.with(|d| {
                online += usize::from(d.online);
                alerts += usize::from(!d.alerts.is_empty());
                updating += usize::from(matches!(
                    d.update.state,
                    UpdateState::Pending | UpdateState::Downloading
                ));
            });
        }
        (devices.len(), online, alerts, updating)
    });
    // Location sections, only when some device has a location.
    let sections = Memo::new(move |_| {
        let devices = fleet.all_devices();
        let mut sections: Vec<(String, Vec<String>)> = Vec::new();
        for device in devices {
            let (location, id) = device.with(|d| (d.location.clone(), d.id.clone()));
            match sections.last_mut() {
                Some((l, ids)) if *l == location => ids.push(id),
                _ => sections.push((location, vec![id])),
            }
        }
        sections
    });
    let grouped = move || sections.with(|s| s.iter().any(|(l, _)| !l.is_empty()));

    view! {
        <PageHeader title="Devices" />
        <div class="page">
            <Show
                when=move || fleet.loaded.get()
                fallback=|| view! { <div class="skeleton-grid"><div></div><div></div></div> }
            >
                <Show when=move || { counts.get().0 > 0 } fallback=EmptyFleet>
                    <div class="stats">
                        <div class="stat">
                            <span class="stat-label">"Online"</span>
                            <span class="stat-value">
                                {move || counts.get().1}
                                <small>{move || format!(" of {}", counts.get().0)}</small>
                            </span>
                        </div>
                        <div class="stat" class:hot=move || { counts.get().2 > 0 }>
                            <span class="stat-label">"Alerts"</span>
                            <span class="stat-value">{move || counts.get().2}</span>
                        </div>
                        <div class="stat" class:busy=move || { counts.get().3 > 0 }>
                            <span class="stat-label">"Updating"</span>
                            <span class="stat-value">{move || counts.get().3}</span>
                        </div>
                    </div>
                    <div class="device-sections" class:grouped=grouped>
                    <For
                        each=move || sections.get()
                        key=|(location, ids)| (location.clone(), ids.clone())
                        let:section
                    >
                        <section class="device-section">
                            <Show when=grouped>
                                <h2 class="section-heading">
                                    {if section.0.is_empty() { "No location".to_string() } else { section.0.clone() }}
                                </h2>
                            </Show>
                            <div class="device-grid">
                                {section.1.iter().map(|id| view! { <DeviceCard id=id.clone() /> }).collect_view()}
                            </div>
                        </section>
                    </For>
                    </div>
                </Show>
            </Show>
        </div>
    }
}

#[component]
fn EmptyFleet() -> impl IntoView {
    view! {
        <div class="empty-state">
            <Svg icon=Icon::Devices class="empty-icon" />
            <h2>"No devices yet"</h2>
            <p>
                "A device appears here on its first check-in. Flash an ull-esp-rs example with "
                <code>"FLEET_BASE_URL"</code> " set to this server, for example "
                <code>"examples/sht3x-ssd1306"</code> "."
            </p>
        </div>
    }
}

#[component]
fn DeviceCard(id: String) -> impl IntoView {
    let fleet = use_fleet();
    let Some(device) = fleet.device_untracked(&id) else {
        return ().into_any();
    };
    let device = RwSignal::from(device);
    let status = Memo::new(move |_| device.with(Status::of));
    let fresh = flash_on_change(Signal::derive(move || device.with(|d| d.last_seen)));
    let href = format!("/devices/{id}");
    let readings = Memo::new(move |_| {
        device.with(|d| {
            d.readings
                .iter()
                .take(CARD_READINGS)
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        })
    });

    view! {
        <A href=href attr:class="card device-card" attr:data-status=move || status.get().key()>
            <header class="device-head">
                <span class="status-dot" class:fresh=move || fresh.get()></span>
                <div class="device-title">
                    <span class="device-name">{move || device.with(|d| d.display_name().to_string())}</span>
                    <span class="device-sub">{move || device.with(subtitle)}</span>
                </div>
                <SignalBars bars=Signal::derive(move || device.with(|d| if d.online { format::signal_bars(d.rssi_dbm) } else { 0 }))/>
            </header>
            <div class="card-readings">
                <For each=move || readings.get() key=|name| name.clone() let:name>
                    <CardReading device=device name=name />
                </For>
                <Show when=move || readings.with(Vec::is_empty)>
                    <p class="muted">"No readings"</p>
                </Show>
            </div>
            <footer class="device-foot">
                <CardFooter device=device />
            </footer>
        </A>
    }
    .into_any()
}

fn subtitle(d: &DeviceView) -> String {
    if d.name.is_empty() {
        d.project.clone()
    } else {
        format!("{} · {}", d.project, &d.id[6..])
    }
}

#[component]
fn CardReading(device: RwSignal<DeviceView>, name: String) -> impl IntoView {
    let fleet = use_fleet();
    let metric = StoredValue::new(name.clone());
    let reading = Memo::new(move |_| {
        device.with(|d| metric.with_value(|m| d.reading(m).map(|r| (r.value, r.ts))))
    });
    let alert = Memo::new(move |_| {
        device.with(|d| metric.with_value(|m| d.alerts.iter().any(|a| a.metric == *m)))
    });
    let fresh = flash_on_change(Signal::derive(move || reading.get().map(|r| r.1)));
    let device_id = device.with_untracked(|d| d.id.clone());
    let key = (device_id, name.clone());
    let points = Signal::derive(move || {
        fleet
            .sparklines
            .with(|lines| lines.get(&key).cloned().unwrap_or_default())
    });
    let kind = units::kind(&name);
    view! {
        <div class=format!("card-reading kind-{kind}") class:alerting=move || alert.get()>
            <span class="reading-label">
                {units::label(&name)}
                <Show when=move || alert.get()>
                    <Svg icon=Icon::Alert class="inline-alert" />
                </Show>
            </span>
            <span class="reading-value" class:fresh=move || fresh.get()>
                <span class="num">
                    {move || metric.with_value(|m| reading.get().map(|r| format::number(m, r.0)).unwrap_or_default())}
                </span>
                <span class="unit">{units::symbol(&name)}</span>
            </span>
            <Sparkline points=points now=fleet.now metric=name.clone() />
        </div>
    }
}

#[component]
fn CardFooter(device: RwSignal<DeviceView>) -> impl IntoView {
    let fleet = use_fleet();
    move || {
        let now = fleet.now.get();
        device.with(|d| {
            if !d.online {
                return view! {
                    <span class="foot-text warn">{format!("Offline · last seen {}", format::ago(d.last_seen, now))}</span>
                }
                .into_any();
            }
            match (d.update.state, d.update.progress) {
                (UpdateState::Downloading, Some(p)) if !p.done => {
                    let pct = p.sent as f64 / p.total.max(1) as f64 * 100.0;
                    view! {
                        <div class="foot-progress">
                            <span>{format!("Downloading {}", d.update.target_version.as_deref().map(format::short_version).unwrap_or_default())}</span>
                            <span class="num">{format!("{pct:.0} %")}</span>
                            <div class="bar"><div style=format!("width:{pct:.1}%")></div></div>
                        </div>
                    }
                    .into_any()
                }
                (UpdateState::Downloading, _) => view! {
                    <span class="foot-text busy">"Installing and restarting"</span>
                }
                .into_any(),
                (UpdateState::Pending, _) => view! {
                    <span class="foot-text busy">{format!(
                        "Update to {} queued",
                        d.update.target_version.as_deref().map(format::short_version).unwrap_or_default()
                    )}</span>
                }
                .into_any(),
                (UpdateState::Failed, _) => view! {
                    <span class="foot-text bad">"Update failed"</span>
                }
                .into_any(),
                (UpdateState::None, _) => view! {
                    <span class="foot-text">
                        {format::ago(d.last_seen, now)}
                        <span class="foot-sep">"·"</span>
                        {format::short_version(&d.version)}
                    </span>
                }
                .into_any(),
            }
        })
    }
}

/// True for a moment after `value` changes (not on first render).
pub fn flash_on_change<T>(value: Signal<T>) -> ReadSignal<bool>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let (flash, set_flash) = signal(false);
    let previous = StoredValue::new(None::<T>);
    let generation = StoredValue::new(0u32);
    Effect::new(move |_| {
        let current = value.get();
        let changed = previous.with_value(|p| p.as_ref().is_some_and(|p| *p != current));
        previous.set_value(Some(current));
        if changed {
            set_flash.set(false);
            generation.update_value(|g| *g += 1);
            let mine = generation.get_value();
            // Restart the animation on the next frame.
            request_animation_frame(move || set_flash.set(true));
            set_timeout(
                move || {
                    if generation.try_get_value() == Some(mine) {
                        set_flash.set(false);
                    }
                },
                std::time::Duration::from_millis(1400),
            );
        }
    });
    flash
}
