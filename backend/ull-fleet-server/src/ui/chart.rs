//! Line charts as inline SVG, drawn on the server.

use maud::{Markup, html};

use crate::units;

const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 132.0;
const PLOT_LEFT: f64 = 34.0;
const PLOT_RIGHT: f64 = WIDTH - 4.0;
const PLOT_TOP: f64 = 8.0;
const PLOT_BOTTOM: f64 = 108.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub key: &'static str,
    pub label: &'static str,
    pub seconds: i64,
    pub bucket_s: i64,
}

pub const RANGES: [Range; 3] = [
    Range {
        key: "6h",
        label: "6 h",
        seconds: 6 * 3600,
        bucket_s: 300,
    },
    Range {
        key: "24h",
        label: "24 h",
        seconds: 24 * 3600,
        bucket_s: 900,
    },
    Range {
        key: "7d",
        label: "7 d",
        seconds: 7 * 86_400,
        bucket_s: 3600,
    },
];

pub fn range(key: Option<&str>) -> Range {
    RANGES
        .into_iter()
        .find(|range| Some(range.key) == key)
        .unwrap_or(RANGES[1])
}

/// Axis bounds and tick step that cover `min..=max` with round numbers.
fn nice_scale(min: f64, max: f64, min_span: f64) -> (f64, f64, f64) {
    let (mut lo, mut hi) = (min, max);
    if hi - lo < min_span {
        let mid = (lo + hi) / 2.0;
        lo = mid - min_span / 2.0;
        hi = mid + min_span / 2.0;
    }
    let raw = (hi - lo) / 2.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|factor| factor * magnitude)
        .find(|step| *step >= raw)
        .unwrap_or(10.0 * magnitude);
    let lo = (lo / step).floor() * step;
    let hi = ((hi / step).ceil() * step).max(lo + step);
    (lo, hi, step)
}

fn tick_label(value: f64, step: f64) -> String {
    let decimals = if step.fract().abs() < 1e-9 {
        0
    } else if (step * 10.0).fract().abs() < 1e-9 {
        1
    } else {
        2
    };
    let text = format!("{value:.decimals$}");
    if text
        .trim_start_matches('-')
        .bytes()
        .all(|b| b == b'0' || b == b'.')
    {
        text.trim_start_matches('-').to_string()
    } else {
        text
    }
}

/// Smallest vertical span shown, so sensor noise does not fill the chart.
fn min_span(metric: &str) -> f64 {
    match units::symbol(metric) {
        "°C" => 2.0,
        "%" => 5.0,
        "dBm" => 10.0,
        _ => 1.0,
    }
}

/// `points` are `(bucket start, value)` pairs, oldest first, within `range`.
pub fn line_chart(metric: &str, points: &[(i64, f64)], range: Range, now: i64) -> Markup {
    let start = now - range.seconds;
    let x = |ts: i64| {
        let t = (ts + range.bucket_s / 2 - start) as f64 / range.seconds as f64;
        PLOT_LEFT + t.clamp(0.0, 1.0) * (PLOT_RIGHT - PLOT_LEFT)
    };

    if points.is_empty() {
        return html! {
            div.chart-empty { "No data in the last " (range.label) }
        };
    }

    let min = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi, step) = nice_scale(min, max, min_span(metric));
    let y = |value: f64| PLOT_BOTTOM - (value - lo) / (hi - lo) * (PLOT_BOTTOM - PLOT_TOP);

    // Break the line where buckets are missing (device offline).
    let mut path = String::new();
    let mut singles = Vec::new();
    let mut previous: Option<i64> = None;
    for (index, &(ts, value)) in points.iter().enumerate() {
        let joined = previous.is_some_and(|prev| ts - prev <= range.bucket_s * 2);
        let next_joined = points
            .get(index + 1)
            .is_some_and(|&(next, _)| next - ts <= range.bucket_s * 2);
        if !joined && !next_joined {
            singles.push((x(ts), y(value)));
        }
        path.push_str(&format!(
            "{}{:.1},{:.1}",
            if joined { "L" } else { "M" },
            x(ts),
            y(value)
        ));
        previous = Some(ts);
    }
    let last = points.last().map(|&(ts, value)| (x(ts), y(value)));

    let mut ticks = Vec::new();
    let mut tick = lo;
    while tick <= hi + step * 0.01 {
        ticks.push(tick);
        tick += step;
    }
    let label = units::label(metric);
    let symbol = units::symbol(metric);

    html! {
        svg.chart viewBox=(format!("0 0 {WIDTH} {HEIGHT}")) role="img"
            aria-label=(format!("{label}, last {}, {} to {} {symbol}", range.label, tick_label(min, step / 10.0), tick_label(max, step / 10.0))) {
            @for tick in &ticks {
                line.grid x1=(PLOT_LEFT) x2=(PLOT_RIGHT) y1=(format!("{:.1}", y(*tick))) y2=(format!("{:.1}", y(*tick))) {}
                text.axis x=(PLOT_LEFT - 6.0) y=(format!("{:.1}", y(*tick) + 3.5)) text-anchor="end" { (tick_label(*tick, step)) }
            }
            text.axis x=(PLOT_LEFT) y=(HEIGHT - 6.0) text-anchor="start" { "−" (range.label) }
            text.axis x=(PLOT_RIGHT) y=(HEIGHT - 6.0) text-anchor="end" { "now" }
            path.line d=(path) {}
            @for (cx, cy) in &singles {
                circle.point cx=(format!("{cx:.1}")) cy=(format!("{cy:.1}")) r="1.75" {}
            }
            @if let Some((cx, cy)) = last {
                circle.last cx=(format!("{cx:.1}")) cy=(format!("{cy:.1}")) r="3" {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_covers_data_with_round_ticks() {
        let (lo, hi, step) = nice_scale(21.3, 26.8, 2.0);
        assert!(lo <= 21.3 && hi >= 26.8);
        assert_eq!(step, 5.0);
        let (lo, hi, _) = nice_scale(22.0, 22.1, 2.0);
        assert!(hi - lo >= 2.0);
    }

    #[test]
    fn gaps_break_the_line() {
        let range = RANGES[0];
        let now = 100_000;
        let start = now - range.seconds;
        let points = [
            (start, 20.0),
            (start + 300, 21.0),
            (start + 3000, 22.0),
            (start + 3300, 22.5),
        ];
        let svg = line_chart("temperature_c", &points, range, now).into_string();
        let d = svg
            .split(" d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(d.matches('M').count(), 2, "{d}");
    }
}
