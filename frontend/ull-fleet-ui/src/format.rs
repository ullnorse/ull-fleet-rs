//! Text for times, durations and readings.

use ull_fleet_types::units;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn date(ts: i64) -> js_sys::Date {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0))
}

/// Seconds since the epoch on this device's clock.
pub fn browser_now() -> f64 {
    js_sys::Date::now() / 1000.0
}

/// "14:05" in local time.
pub fn clock(ts: i64) -> String {
    let d = date(ts);
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

/// "Nov 23".
pub fn day(ts: i64) -> String {
    let d = date(ts);
    format!("{} {}", MONTHS[d.get_month() as usize % 12], d.get_date())
}

/// "Nov 23, 14:05".
pub fn day_clock(ts: i64) -> String {
    format!("{}, {}", day(ts), clock(ts))
}

/// Local midnight of the day containing `ts`.
pub fn day_start(ts: i64) -> i64 {
    let d = date(ts);
    d.set_hours(0);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    (d.get_time() / 1000.0) as i64
}

/// "Today", "Yesterday", or "Mon, Nov 21".
pub fn day_heading(ts: i64, now: i64) -> String {
    let today = day_start(now);
    let start = day_start(ts);
    if start == today {
        "Today".into()
    } else if start == day_start(today - 43_200) {
        "Yesterday".into()
    } else {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        format!("{}, {}", DAYS[date(ts).get_day() as usize % 7], day(ts))
    }
}

/// "45 s", "12 min", "3 h 20 min", "2 d 4 h".
pub fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => match s % 3600 / 60 {
            0 => format!("{} h", s / 3600),
            m => format!("{} h {m} min", s / 3600),
        },
        _ => match s % 86_400 / 3600 {
            0 => format!("{} d", s / 86_400),
            h => format!("{} d {h} h", s / 86_400),
        },
    }
}

/// "just now", "12 s ago", "5 min ago", then the date.
pub fn ago(ts: i64, now: i64) -> String {
    let diff = now - ts;
    if diff < 3 {
        "just now".into()
    } else if diff < 86_400 {
        format!("{} ago", duration(diff))
    } else {
        day_clock(ts)
    }
}

pub fn number(metric: &str, value: f64) -> String {
    units::number(metric, value)
}

pub fn with_unit(metric: &str, value: f64) -> String {
    units::format(metric, value)
}

/// "1.2 MB", "840 KB".
pub fn bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1_000_000.0 {
        format!("{:.2} MB", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.0} KB", n / 1_000.0)
    } else {
        format!("{n} B")
    }
}

/// Firmware version without the build stamp: "0.1.0+c8945d7-1795389495" →
/// ("0.1.0", Some("c8945d7")), build time aside.
pub struct Version<'a> {
    pub base: &'a str,
    pub rev: Option<&'a str>,
    pub built: Option<i64>,
}

pub fn version(text: &str) -> Version<'_> {
    let (base, meta) = match text.split_once('+') {
        Some((base, meta)) => (base, Some(meta)),
        None => (text, None),
    };
    let (rev, built) = match meta.and_then(|meta| meta.rsplit_once('-')) {
        Some((rev, epoch)) => (Some(rev), epoch.parse().ok()),
        None => (meta, None),
    };
    Version { base, rev, built }
}

/// "0.1.0 · c8945d7".
pub fn short_version(text: &str) -> String {
    let v = version(text);
    match v.rev {
        Some(rev) => format!("{} · {rev}", v.base),
        None => v.base.to_string(),
    }
}

/// Shortens full build versions ("0.1.0+c8945d7-1795389495") inside an
/// event message to "0.1.0 · c8945d7".
pub fn tidy_message(message: &str) -> String {
    message
        .split(' ')
        .map(|word| {
            let looks_like_build =
                word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('+');
            if looks_like_build {
                short_version(word)
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Wi-Fi signal in 0..=4 bars.
pub fn signal_bars(rssi: Option<i64>) -> u8 {
    match rssi {
        None => 0,
        Some(r) if r >= -55 => 4,
        Some(r) if r >= -65 => 3,
        Some(r) if r >= -75 => 2,
        Some(_) => 1,
    }
}

pub fn signal_label(rssi: Option<i64>) -> &'static str {
    match signal_bars(rssi) {
        4 => "Excellent",
        3 => "Good",
        2 => "Fair",
        1 => "Weak",
        _ => "Unknown",
    }
}

/// Dew point (°C) from temperature (°C) and relative humidity (%), Magnus formula.
pub fn dew_point(temperature: f64, humidity: f64) -> Option<f64> {
    if !(1.0..=100.0).contains(&humidity) {
        return None;
    }
    let (a, b) = (17.62, 243.12);
    let gamma = (humidity / 100.0).ln() + a * temperature / (b + temperature);
    Some(b * gamma / (a - gamma))
}

/// The device's reported restart reason, for the details card.
pub fn restart_reason(reason: &str) -> &'static str {
    match reason {
        "panic" => "Crash (panic)",
        "stall" => "Watchdog: a task stopped",
        "watchdog" => "Crash (hardware watchdog)",
        "brownout" => "Supply voltage dip",
        "power_on" => "Power on",
        "update" => "Update installed",
        "rollback" => "Rolled back",
        "requested" => "On request",
        "deep_sleep" => "Wake from sleep",
        _ => "Software reset",
    }
}

/// Reasons that mean the firmware failed.
pub fn is_crash(reason: &str) -> bool {
    matches!(reason, "panic" | "stall" | "watchdog")
}
