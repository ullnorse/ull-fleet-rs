//! Time-series charts drawn as SVG: a full chart with a min/max band and a
//! touch/mouse crosshair, and a sparkline for cards.
//!
//! The SVG stretches to its box (`preserveAspectRatio="none"`, strokes do not
//! scale); text, the crosshair dot and the tooltip are HTML on top, placed in
//! percent, so nothing is distorted.

use std::sync::atomic::{AtomicU32, Ordering};

use leptos::prelude::*;
use ull_fleet_types::{Point, Series};

use crate::format;

pub(crate) const W: f64 = 1000.0;
pub(crate) const H: f64 = 300.0;
/// Vertical room kept free above and below the data, as a fraction of H.
pub(crate) const PAD: f64 = 0.1;

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

pub(crate) fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// 1, 2, 2.5 or 5 times a power of ten, at least `raw`.
fn nice_step(raw: f64) -> f64 {
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let power = 10f64.powf(raw.log10().floor());
    let fraction = raw / power;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 2.5 {
        2.5
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * power
}

/// Y domain and tick values: about four ticks, at least `min_span` wide.
pub(crate) fn y_scale(lo: f64, hi: f64, min_span: f64) -> (f64, f64, Vec<f64>) {
    let (mut lo, mut hi) = (lo, hi);
    if hi - lo < min_span {
        let mid = (lo + hi) / 2.0;
        lo = mid - min_span / 2.0;
        hi = mid + min_span / 2.0;
    }
    let step = nice_step((hi - lo) / 3.0);
    let start = (lo / step).floor() * step;
    let end = (hi / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut tick = start;
    while tick <= end + step * 0.001 {
        ticks.push(tick);
        tick += step;
    }
    (start, end, ticks)
}

/// The smallest span (in the metric's unit) worth showing: keeps a flat
/// line from turning sensor noise into mountains.
pub(crate) fn min_span(metric: &str) -> f64 {
    match ull_fleet_types::units::decimals(metric) {
        0 => 4.0,
        1 => 1.0,
        _ => 0.2,
    }
}

/// Time ticks aligned to local clock boundaries.
pub(crate) fn x_ticks(from: i64, to: i64) -> Vec<(i64, String)> {
    const STEPS: [i64; 12] = [
        300, 600, 900, 1800, 3600, 7200, 10_800, 21_600, 43_200, 86_400, 172_800, 604_800,
    ];
    let span = (to - from).max(1);
    let step = STEPS
        .into_iter()
        .find(|step| span / step <= 4)
        .unwrap_or(604_800);
    // Offset of local time from UTC, in seconds.
    let offset = -(js_sys::Date::new_0().get_timezone_offset() * 60.0) as i64;
    let first = ((from + offset) / step + 1) * step - offset;
    let mut ticks = Vec::new();
    let mut t = first;
    while t < to {
        let label = if step >= 86_400 {
            format::day(t)
        } else {
            format::clock(t)
        };
        ticks.push((t, label));
        t += step;
    }
    ticks
}

#[derive(PartialEq)]
struct Layout {
    from: i64,
    to: i64,
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    segments: Vec<Vec<Point>>,
    bucket_s: i64,
    /// Alert limits inside the plotted range.
    limits: Vec<f64>,
}

impl Layout {
    fn new(series: &Series, metric: &str, gap_s: i64, limits: &[f64]) -> Option<Self> {
        if series.points.is_empty() {
            return None;
        }
        let mut lo = series
            .points
            .iter()
            .map(|p| p.min)
            .fold(f64::INFINITY, f64::min);
        let mut hi = series
            .points
            .iter()
            .map(|p| p.max)
            .fold(f64::NEG_INFINITY, f64::max);
        // Show a limit when it is near the data; a far one would flatten the line.
        let reach = (hi - lo).max(min_span(metric));
        let limits: Vec<f64> = limits
            .iter()
            .copied()
            .filter(|l| *l >= lo - reach && *l <= hi + reach)
            .collect();
        for limit in &limits {
            lo = lo.min(*limit);
            hi = hi.max(*limit);
        }
        let (lo, hi, ticks) = y_scale(lo, hi, min_span(metric));
        // A gap longer than this breaks the line.
        let max_gap = (series.bucket_s * 3).max(gap_s);
        let mut segments: Vec<Vec<Point>> = Vec::new();
        for point in &series.points {
            match segments.last_mut() {
                Some(segment)
                    if segment
                        .last()
                        .is_some_and(|last| point.t - last.t <= max_gap) =>
                {
                    segment.push(*point)
                }
                _ => segments.push(vec![*point]),
            }
        }
        Some(Self {
            from: series.from,
            to: series.to,
            lo,
            hi,
            ticks,
            segments,
            bucket_s: series.bucket_s,
            limits,
        })
    }

    /// Bucket centre, clamped into the window.
    fn x(&self, t: i64) -> f64 {
        let center = (t + self.bucket_s / 2).min(self.to);
        (center - self.from) as f64 / (self.to - self.from).max(1) as f64 * W
    }

    fn y(&self, value: f64) -> f64 {
        let frac = (value - self.lo) / (self.hi - self.lo).max(f64::EPSILON);
        H * (1.0 - PAD) - frac * H * (1.0 - 2.0 * PAD)
    }

    fn line(&self) -> String {
        let mut d = String::new();
        for segment in &self.segments {
            for (i, p) in segment.iter().enumerate() {
                let cmd = if i == 0 { 'M' } else { 'L' };
                d.push_str(&format!("{cmd}{:.1} {:.1}", self.x(p.t), self.y(p.avg)));
            }
            if segment.len() == 1 {
                // A lone reading: a dot made of a round-capped zero-length line.
                d.push_str("h0.01");
            }
        }
        d
    }

    fn area(&self) -> String {
        let mut d = String::new();
        let base = H;
        for segment in self.segments.iter().filter(|s| s.len() > 1) {
            let first = self.x(segment[0].t);
            let last = self.x(segment[segment.len() - 1].t);
            d.push_str(&format!("M{first:.1} {base:.1}"));
            for p in segment {
                d.push_str(&format!("L{:.1} {:.1}", self.x(p.t), self.y(p.avg)));
            }
            d.push_str(&format!("L{last:.1} {base:.1}Z"));
        }
        d
    }

    fn band(&self) -> String {
        let mut d = String::new();
        for segment in self.segments.iter().filter(|s| s.len() > 1) {
            for (i, p) in segment.iter().enumerate() {
                let cmd = if i == 0 { 'M' } else { 'L' };
                d.push_str(&format!("{cmd}{:.1} {:.1}", self.x(p.t), self.y(p.max)));
            }
            for p in segment.iter().rev() {
                d.push_str(&format!("L{:.1} {:.1}", self.x(p.t), self.y(p.min)));
            }
            d.push('Z');
        }
        d
    }

    fn nearest(&self, t: f64) -> Option<Point> {
        self.segments
            .iter()
            .flatten()
            .min_by(|a, b| {
                let da = ((a.t + self.bucket_s / 2) as f64 - t).abs();
                let db = ((b.t + self.bucket_s / 2) as f64 - t).abs();
                da.total_cmp(&db)
            })
            .copied()
    }
}

/// A metric over time: line of bucket averages, min/max band, crosshair.
#[component]
pub fn LineChart(
    #[prop(into)] series: Signal<Option<Series>>,
    metric: String,
    /// The series is being refetched: dim the old frame instead of blanking it.
    #[prop(into)]
    loading: Signal<bool>,
    /// Longest normal gap between readings, in seconds.
    #[prop(into)]
    gap_s: Signal<i64>,
    /// Alert limits, drawn as dashed lines.
    #[prop(into)]
    limits: Signal<Vec<f64>>,
) -> impl IntoView {
    let id = next_id();
    let gradient = format!("fill-{id}");
    let metric = StoredValue::new(metric);
    let layout = Memo::new(move |_| {
        series.with(|series| {
            series
                .as_ref()
                .and_then(|s| {
                    metric.with_value(|m| limits.with(|l| Layout::new(s, m, gap_s.get(), l)))
                })
                .map(std::sync::Arc::new)
        })
    });
    let hover = RwSignal::new(None::<Point>);
    let box_ref = NodeRef::<leptos::html::Div>::new();

    let pick = move |client_x: f64| {
        let Some(el) = box_ref.get_untracked() else {
            return;
        };
        let rect = el.get_bounding_client_rect();
        let frac = ((client_x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        layout.with_untracked(|layout| {
            if let Some(layout) = layout {
                let t = layout.from as f64 + frac * (layout.to - layout.from) as f64;
                hover.set(layout.nearest(t));
            }
        });
    };

    let gradient_url = format!("url(#{gradient})");
    view! {
        <div class="chart" class:dim=move || loading.get()>
            <div
                class="chart-plot"
                node_ref=box_ref
                on:pointermove=move |ev| pick(f64::from(ev.client_x()))
                on:pointerdown=move |ev| pick(f64::from(ev.client_x()))
                on:pointerleave=move |_| hover.set(None)
                on:pointercancel=move |_| hover.set(None)
            >
                {move || match layout.get() {
                    None => {
                        view! {
                            <div class="chart-empty">
                                {move || if series.with(Option::is_none) { "Loading" } else { "No readings in this range" }}
                            </div>
                        }
                            .into_any()
                    }
                    Some(layout) => {
                        let grid = layout
                            .ticks
                            .iter()
                            .map(|tick| {
                                let y = layout.y(*tick);
                                view! { <line class="grid" x1="0" x2=W y1=y y2=y /> }
                            })
                            .collect_view();
                        let limit_lines = layout
                            .limits
                            .iter()
                            .map(|limit| {
                                let y = layout.y(*limit);
                                view! { <line class="limit" x1="0" x2=W y1=y y2=y /> }
                            })
                            .collect_view();
                        let labels = layout
                            .ticks
                            .iter()
                            .map(|tick| {
                                let top = layout.y(*tick) / H * 100.0;
                                let text = metric.with_value(|m| format::number(m, *tick));
                                view! {
                                    <span class="y-label" style=format!("top:{top:.2}%")>
                                        {text}
                                    </span>
                                }
                            })
                            .collect_view();
                        let last = layout.segments.last().and_then(|s| s.last()).copied();
                        let live_dot = last
                            .filter(|p| layout.to - p.t <= layout.bucket_s * 3)
                            .map(|p| {
                                let left = layout.x(p.t) / W * 100.0;
                                let top = layout.y(p.avg) / H * 100.0;
                                view! {
                                    <span
                                        class="live-dot"
                                        style=format!("left:{left:.2}%;top:{top:.2}%")
                                    ></span>
                                }
                            });
                        view! {
                            <svg viewBox=format!("0 0 {W} {H}") preserveAspectRatio="none">
                                <defs>
                                    <linearGradient id=gradient.clone() x1="0" x2="0" y1="0" y2="1">
                                        <stop offset="0%" class="stop-top" />
                                        <stop offset="100%" class="stop-bottom" />
                                    </linearGradient>
                                </defs>
                                {grid}
                                <path class="area" d=layout.area() fill=gradient_url.clone() />
                                <path class="band" d=layout.band() />
                                {limit_lines}
                                <path class="line" d=layout.line() />
                            </svg>
                            {labels}
                            {live_dot}
                        }
                            .into_any()
                    }
                }}
                {move || {
                    let point = hover.get()?;
                    let layout = layout.get()?;
                    let left = layout.x(point.t) / W * 100.0;
                    let top = layout.y(point.avg) / H * 100.0;
                    let (value, range) = metric
                        .with_value(|m| {
                            let range = if point.max - point.min >= 10f64.powi(-(ull_fleet_types::units::decimals(m) as i32)) {
                                Some(format!("{} – {}", format::number(m, point.min), format::with_unit(m, point.max)))
                            } else {
                                None
                            };
                            (format::with_unit(m, point.avg), range)
                        });
                    let when = if layout.bucket_s >= 3600 {
                        format::day_clock(point.t)
                    } else {
                        format::clock(point.t + layout.bucket_s / 2)
                    };
                    let side = if left > 60.0 { "left" } else { "right" };
                    Some(
                        view! {
                            <div class="crosshair" style=format!("left:{left:.2}%")></div>
                            <span class="hover-dot" style=format!("left:{left:.2}%;top:{top:.2}%")></span>
                            <div
                                class=format!("tip tip-{side}")
                                style=format!("left:{left:.2}%")
                            >
                                <strong>{value}</strong>
                                <span>{when}</span>
                                {range.map(|r| view! { <span class="tip-range">{r}</span> })}
                            </div>
                        },
                    )
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
                                    let left = (t - layout.from) as f64
                                        / (layout.to - layout.from).max(1) as f64 * 100.0;
                                    view! {
                                        <span style=format!("left:{left:.2}%")>{label}</span>
                                    }
                                })
                                .collect_view()
                        })
                }}
            </div>
        </div>
    }
}

/// 24 h trend for a card. `points` are `(bucket start, sum, count)`.
#[component]
pub fn Sparkline(
    #[prop(into)] points: Signal<Vec<(i64, f64, u32)>>,
    #[prop(into)] now: Signal<i64>,
    metric: String,
) -> impl IntoView {
    let id = next_id();
    let gradient = format!("spark-{id}");
    let min_value_span = min_span(&metric) / 2.0;
    let paths = Memo::new(move |_| {
        let points = points.get();
        // Redraw every 5 minutes as the window slides.
        let to = now.get() / 300 * 300 + 300;
        // The last 24 h, or less for a device with a shorter history (at
        // least 1 h), so a new device's trend is not squeezed to the edge.
        let first = points
            .iter()
            .filter(|p| p.2 > 0)
            .map(|p| p.0)
            .find(|t| *t >= to - 86_400);
        let from = first.map_or(to - 86_400, |t| t.min(to - 3_600));
        let span = (to - from) as f64;
        let values: Vec<(i64, f64, f64)> = points
            .iter()
            .filter(|p| p.0 >= from && p.2 > 0)
            .map(|p| {
                let x = (p.0 + 900 - from) as f64 / span * 100.0;
                (p.0, x.min(100.0), p.1 / f64::from(p.2))
            })
            .collect();
        if values.len() < 2 {
            return None;
        }
        let lo = values.iter().map(|v| v.2).fold(f64::INFINITY, f64::min);
        let hi = values.iter().map(|v| v.2).fold(f64::NEG_INFINITY, f64::max);
        let (lo, hi) = if hi - lo < min_value_span {
            let mid = (hi + lo) / 2.0;
            (mid - min_value_span / 2.0, mid + min_value_span / 2.0)
        } else {
            (lo, hi)
        };
        let y = |v: f64| 28.0 - (v - lo) / (hi - lo) * 24.0;
        let mut line = String::new();
        let mut prev_t = None::<i64>;
        for (t, x, v) in &values {
            // Break the line over gaps of more than 1.5 hours.
            let cmd = match prev_t {
                Some(pt) if t - pt <= 5_400 => 'L',
                _ => 'M',
            };
            line.push_str(&format!("{cmd}{x:.2} {:.2}", y(*v)));
            prev_t = Some(*t);
        }
        let first = values[0].1;
        let last = values[values.len() - 1].1;
        let area = format!("{line}L{last:.2} 30L{first:.2} 30Z").replacen('M', "M", 1);
        Some((line, area))
    });
    let fill = format!("url(#{gradient})");
    view! {
        <svg class="spark" viewBox="0 0 100 30" preserveAspectRatio="none" aria-hidden="true">
            <defs>
                <linearGradient id=gradient x1="0" x2="0" y1="0" y2="1">
                    <stop offset="0%" class="stop-top" />
                    <stop offset="100%" class="stop-bottom" />
                </linearGradient>
            </defs>
            {move || {
                paths
                    .get()
                    .map(|(line, area)| {
                        let area = if line.matches('M').count() == 1 { area } else { String::new() };
                        view! {
                            <path class="area" d=area fill=fill.clone() />
                            <path class="line" d=line />
                        }
                    })
            }}
        </svg>
    }
}
