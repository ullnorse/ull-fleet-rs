//! Trends: one metric across every device that reports it, on one chart.

use leptos::prelude::*;
use ull_fleet_types::{Range, Series, units};

use crate::api;
use crate::components::multichart::{Line, Lines, MultiChart, SLOTS};
use crate::components::shell::PageHeader;
use crate::components::widgets::Segmented;
use crate::format;
use crate::store::use_fleet;

const METRIC_KEY: &str = "trends-metric";
const RANGE_KEY: &str = "trends-range";

fn stored(key: &str) -> Option<String> {
    window()
        .local_storage()
        .ok()
        .flatten()?
        .get_item(key)
        .ok()
        .flatten()
}

fn store(key: &str, value: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.set_item(key, value);
    }
}

/// Order of metric kinds in the picker.
fn kind_rank(metric: &str) -> usize {
    [
        "temperature",
        "humidity",
        "pressure",
        "air",
        "light",
        "power",
        "signal",
        "other",
    ]
    .iter()
    .position(|k| *k == units::kind(metric))
    .unwrap_or(9)
}

#[component]
pub fn TrendsPage() -> impl IntoView {
    let fleet = use_fleet();

    // Every metric any device reports.
    let metrics = Memo::new(move |_| {
        let mut names: Vec<String> = Vec::new();
        for device in fleet.all_devices() {
            device.with(|d| {
                for r in &d.readings {
                    if !names.contains(&r.name) {
                        names.push(r.name.clone());
                    }
                }
            });
        }
        names.sort_by(|a, b| kind_rank(a).cmp(&kind_rank(b)).then(a.cmp(b)));
        names
    });
    let chosen = RwSignal::new(stored(METRIC_KEY));
    let metric = Memo::new(move |_| {
        let names = metrics.get();
        chosen
            .get()
            .filter(|m| names.contains(m))
            .or_else(|| names.first().cloned())
    });
    let range = RwSignal::new(
        stored(RANGE_KEY)
            .and_then(|r| Range::parse(&r))
            .unwrap_or(Range::Day),
    );
    Effect::new(move |_| store(RANGE_KEY, range.get().as_str()));

    // Devices reporting the metric, each with its fixed color slot: the
    // device's position in the fleet, so switching metric keeps colors.
    let devices = Memo::new(move |_| {
        let Some(metric) = metric.get() else {
            return Vec::new();
        };
        fleet
            .all_devices()
            .iter()
            .enumerate()
            .filter_map(|(i, device)| {
                device.with(|d| {
                    d.reading(&metric).map(|r| {
                        (
                            d.id.clone(),
                            d.display_name().to_string(),
                            i + 1,
                            r.value,
                            d.online,
                        )
                    })
                })
            })
            .collect::<Vec<_>>()
    });
    let hidden = RwSignal::new(Vec::<String>::new());
    let data = RwSignal::new(None::<Lines>);
    let loading = RwSignal::new(false);
    let generation = StoredValue::new(0u32);
    // Refresh once a minute while open; live readings arrive that often or less.
    let minute = Memo::new(move |_| fleet.now.get() / 60);

    // Which lines to fetch; unchanged by new readings.
    let line_ids = Memo::new(move |_| {
        devices.with(|d| {
            d.iter()
                .filter(|d| d.2 <= SLOTS)
                .map(|(id, name, slot, _, _)| (id.clone(), name.clone(), *slot))
                .collect::<Vec<_>>()
        })
    });

    Effect::new(move |previous: Option<(Option<String>, Range)>| {
        let key = (metric.get(), range.get());
        minute.track();
        let ids = line_ids.get();
        let Some(metric_name) = key.0.clone() else {
            return key;
        };
        if previous.as_ref() != Some(&key) {
            loading.set(true);
        }
        generation.update_value(|g| *g += 1);
        let mine = generation.get_value();
        let range = key.1;
        leptos::task::spawn_local(async move {
            let mut lines = Vec::new();
            let mut window = None;
            for (id, name, slot) in ids {
                let url = format!(
                    "/api/v1/devices/{id}/series?metric={metric_name}&range={}",
                    range.as_str()
                );
                if let Ok(series) = api::get::<Series>(&url).await {
                    window.get_or_insert((series.from, series.to, series.bucket_s));
                    lines.push(Line {
                        id,
                        name,
                        slot,
                        points: series.points,
                    });
                }
            }
            if generation.try_get_value() != Some(mine) {
                return;
            }
            data.set(window.map(|(from, to, bucket_s)| Lines {
                metric: metric_name,
                from,
                to,
                bucket_s,
                lines,
            }));
            loading.set(false);
        });
        key
    });

    let toggle = move |id: String| {
        hidden.update(|h| {
            if let Some(i) = h.iter().position(|x| *x == id) {
                h.remove(i);
            } else {
                h.push(id);
            }
        })
    };

    view! {
        <PageHeader title="Trends" />
        <div class="page trends">
            <Show
                when=move || metrics.with(|m| !m.is_empty())
                fallback=|| view! { <p class="muted">"No readings yet."</p> }
            >
                <div class="chips" role="tablist" aria-label="Metric">
                    <For each=move || metrics.get() key=|m| m.clone() let:name>
                        {
                            let value = name.clone();
                            let current = name.clone();
                            view! {
                                <button
                                    class="chip"
                                    role="tab"
                                    aria-selected=move || (metric.get().as_deref() == Some(current.as_str())).to_string()
                                    class:on={
                                        let name = name.clone();
                                        move || metric.get().as_deref() == Some(name.as_str())
                                    }
                                    on:click=move |_| {
                                        store(METRIC_KEY, &value);
                                        chosen.set(Some(value.clone()));
                                    }
                                >
                                    {units::label(&name)}
                                </button>
                            }
                        }
                    </For>
                </div>
                <section class=move || format!("card trends-card kind-{}", metric.get().as_deref().map(units::kind).unwrap_or("other"))>
                    <div class="toolbar">
                        <h2 class="trends-title">
                            {move || metric.get().map(|m| format!("{} · {}", units::label(&m), units::symbol(&m))).unwrap_or_default()}
                        </h2>
                        <Segmented
                            label="Time range"
                            value=range
                            options=Range::ALL.iter().map(|r| (*r, r.as_str())).collect()
                        />
                    </div>
                    <MultiChart data=data hidden=hidden loading=loading />
                    <ul class="legend">
                        <For
                            each=move || devices.get()
                            key=|d| (d.0.clone(), d.1.clone(), d.2, d.3.to_bits(), d.4)
                            let:device
                        >
                            {
                                let (id, name, slot, value, online) = device;
                                let plotted = slot <= SLOTS;
                                let off = {
                                    let id = id.clone();
                                    Signal::derive(move || hidden.with(|h| h.contains(&id)))
                                };
                                let unit_metric = metric.get_untracked().unwrap_or_default();
                                view! {
                                    <li>
                                        <button
                                            class="legend-row"
                                            class:off=off
                                            disabled=!plotted
                                            aria-pressed=move || (!off.get()).to_string()
                                            on:click={
                                                let id = id.clone();
                                                move |_| toggle(id.clone())
                                            }
                                        >
                                            <i class=format!("swatch c{slot}") class:none=!plotted></i>
                                            <span class="legend-name">{name}</span>
                                            <span class="legend-value" class:stale=!online>
                                                {format::with_unit(&unit_metric, value)}
                                            </span>
                                        </button>
                                    </li>
                                }
                            }
                        </For>
                    </ul>
                </section>
            </Show>
        </div>
    }
}
