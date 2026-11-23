//! One metric across several devices: a line per device in its fixed
//! categorical color, one shared y axis, and a crosshair that lists every
//! device's value at that moment.

use leptos::prelude::*;
use ull_fleet_types::Point;

use crate::components::chart::{H, PAD, W, min_span, x_ticks, y_scale};
use crate::format;

/// Categorical slots (`--c1` … `--c8`). A ninth device is not plotted.
pub const SLOTS: usize = 8;

#[derive(Clone, PartialEq)]
pub struct Line {
    pub id: String,
    pub name: String,
    /// Color slot, 1-based; follows the device, not its rank.
    pub slot: usize,
    pub points: Vec<Point>,
}

#[derive(Clone, PartialEq)]
pub struct Lines {
    pub metric: String,
    pub from: i64,
    pub to: i64,
    pub bucket_s: i64,
    pub lines: Vec<Line>,
}

#[derive(PartialEq)]
struct Layout {
    from: i64,
    to: i64,
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    bucket_s: i64,
}

impl Layout {
    fn new(data: &Lines, hidden: &[String]) -> Option<Self> {
        let visible = data.lines.iter().filter(|l| !hidden.contains(&l.id));
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for point in visible.flat_map(|l| l.points.iter()) {
            lo = lo.min(point.avg);
            hi = hi.max(point.avg);
        }
        if !lo.is_finite() {
            return None;
        }
        let (lo, hi, ticks) = y_scale(lo, hi, min_span(&data.metric));
        Some(Self {
            from: data.from,
            to: data.to,
            lo,
            hi,
            ticks,
            bucket_s: data.bucket_s,
        })
    }

    fn x(&self, t: i64) -> f64 {
        let center = (t + self.bucket_s / 2).min(self.to);
        (center - self.from) as f64 / (self.to - self.from).max(1) as f64 * W
    }

    fn y(&self, value: f64) -> f64 {
        let frac = (value - self.lo) / (self.hi - self.lo).max(f64::EPSILON);
        H * (1.0 - PAD) - frac * H * (1.0 - 2.0 * PAD)
    }

    /// Path of one line, broken where readings are missing.
    fn path(&self, points: &[Point]) -> String {
        let max_gap = self.bucket_s * 3;
        let mut d = String::new();
        let mut previous: Option<i64> = None;
        for p in points {
            let cmd = match previous {
                Some(t) if p.t - t <= max_gap => 'L',
                _ => 'M',
            };
            d.push_str(&format!("{cmd}{:.1} {:.1}", self.x(p.t), self.y(p.avg)));
            previous = Some(p.t);
        }
        d
    }
}

#[component]
pub fn MultiChart(
    #[prop(into)] data: Signal<Option<Lines>>,
    /// Device ids switched off in the legend.
    #[prop(into)]
    hidden: Signal<Vec<String>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    let layout = Memo::new(move |_| {
        data.with(|d| d.as_ref().and_then(|d| hidden.with(|h| Layout::new(d, h))))
            .map(std::sync::Arc::new)
    });
    // Hovered bucket start.
    let hover = RwSignal::new(None::<i64>);
    let box_ref = NodeRef::<leptos::html::Div>::new();

    let pick = move |client_x: f64| {
        let Some(el) = box_ref.get_untracked() else {
            return;
        };
        let rect = el.get_bounding_client_rect();
        let frac = ((client_x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let Some(layout) = layout.get_untracked() else {
            return;
        };
        let t = layout.from as f64 + frac * (layout.to - layout.from) as f64;
        // Buckets are aligned to multiples of the bucket size.
        let bucket = ((t - layout.bucket_s as f64 / 2.0) / layout.bucket_s as f64).round() as i64
            * layout.bucket_s;
        hover.set(Some(bucket));
    };

    view! {
        <div class="chart multi" class:dim=move || loading.get()>
            <div
                class="chart-plot"
                node_ref=box_ref
                on:pointermove=move |ev| pick(f64::from(ev.client_x()))
                on:pointerdown=move |ev| pick(f64::from(ev.client_x()))
                on:pointerleave=move |_| hover.set(None)
                on:pointercancel=move |_| hover.set(None)
            >
                {move || {
                    let Some(layout) = layout.get() else {
                        return view! {
                            <div class="chart-empty">
                                {move || if data.with(Option::is_none) { "Loading" } else { "No readings in this range" }}
                            </div>
                        }
                        .into_any();
                    };
                    let grid = layout
                        .ticks
                        .iter()
                        .map(|tick| {
                            let y = layout.y(*tick);
                            view! { <line class="grid" x1="0" x2=W y1=y y2=y /> }
                        })
                        .collect_view();
                    let metric = data.with(|d| d.as_ref().map(|d| d.metric.clone()).unwrap_or_default());
                    let labels = layout
                        .ticks
                        .iter()
                        .map(|tick| {
                            let top = layout.y(*tick) / H * 100.0;
                            view! {
                                <span class="y-label" style=format!("top:{top:.2}%")>
                                    {format::number(&metric, *tick)}
                                </span>
                            }
                        })
                        .collect_view();
                    let paths = data.with(|d| {
                        hidden.with(|hidden| {
                            d.as_ref()
                                .map(|d| {
                                    d.lines
                                        .iter()
                                        .filter(|l| !hidden.contains(&l.id))
                                        .map(|l| {
                                            view! {
                                                <path class=format!("mline c{}", l.slot) d=layout.path(&l.points) />
                                            }
                                        })
                                        .collect_view()
                                })
                        })
                    });
                    view! {
                        <svg viewBox=format!("0 0 {W} {H}") preserveAspectRatio="none">
                            {grid}
                            {paths}
                        </svg>
                        {labels}
                    }
                    .into_any()
                }}
                {move || {
                    let bucket = hover.get()?;
                    let layout = layout.get()?;
                    let (metric, mut rows) = data.with(|d| {
                        let d = d.as_ref()?;
                        let rows = hidden.with(|hidden| {
                            d.lines
                                .iter()
                                .filter(|l| !hidden.contains(&l.id))
                                .filter_map(|l| {
                                    l.points
                                        .iter()
                                        .find(|p| p.t == bucket)
                                        .map(|p| (l.slot, l.name.clone(), p.avg))
                                })
                                .collect::<Vec<_>>()
                        });
                        Some((d.metric.clone(), rows))
                    })?;
                    if rows.is_empty() {
                        return None;
                    }
                    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
                    let left = layout.x(bucket) / W * 100.0;
                    let side = if left > 55.0 { "left" } else { "right" };
                    let when = if layout.bucket_s >= 3600 {
                        format::day_clock(bucket)
                    } else {
                        format::clock(bucket + layout.bucket_s / 2)
                    };
                    let dots = rows
                        .iter()
                        .map(|(slot, _, value)| {
                            let top = layout.y(*value) / H * 100.0;
                            view! {
                                <span
                                    class=format!("hover-dot c{slot}")
                                    style=format!("left:{left:.2}%;top:{top:.2}%")
                                ></span>
                            }
                        })
                        .collect_view();
                    let list = rows
                        .into_iter()
                        .map(|(slot, name, value)| {
                            view! {
                                <span class="tip-row">
                                    <i class=format!("swatch c{slot}")></i>
                                    <span class="tip-name">{name}</span>
                                    <b>{format::with_unit(&metric, value)}</b>
                                </span>
                            }
                        })
                        .collect_view();
                    Some(view! {
                        <div class="crosshair" style=format!("left:{left:.2}%")></div>
                        {dots}
                        <div class=format!("tip tip-{side} tip-multi") style=format!("left:{left:.2}%")>
                            <span>{when}</span>
                            {list}
                        </div>
                    })
                }}
            </div>
            <div class="x-axis">
                {move || {
                    layout
                        .get()
                        .map(|layout| {
                            x_ticks(layout.from, layout.to)
                                .into_iter()
                                .map(|(t, label)| {
                                    let left = (t - layout.from) as f64 / (layout.to - layout.from).max(1) as f64 * 100.0;
                                    view! { <span style=format!("left:{left:.2}%")>{label}</span> }
                                })
                                .collect_view()
                        })
                }}
            </div>
        </div>
    }
}
