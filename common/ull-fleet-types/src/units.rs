//! Labels and units derived from metric names (`temperature_c`, `humidity_pct`, ...).

struct Unit {
    suffix: &'static str,
    symbol: &'static str,
    decimals: usize,
}

const UNITS: &[Unit] = &[
    Unit {
        suffix: "_c",
        symbol: "°C",
        decimals: 1,
    },
    Unit {
        suffix: "_pct",
        symbol: "%",
        decimals: 0,
    },
    Unit {
        suffix: "_dbm",
        symbol: "dBm",
        decimals: 0,
    },
    Unit {
        suffix: "_hpa",
        symbol: "hPa",
        decimals: 0,
    },
    Unit {
        suffix: "_ppm",
        symbol: "ppm",
        decimals: 0,
    },
    Unit {
        suffix: "_lux",
        symbol: "lx",
        decimals: 0,
    },
    Unit {
        suffix: "_mv",
        symbol: "mV",
        decimals: 0,
    },
    Unit {
        suffix: "_v",
        symbol: "V",
        decimals: 2,
    },
    Unit {
        suffix: "_a",
        symbol: "A",
        decimals: 2,
    },
    Unit {
        suffix: "_w",
        symbol: "W",
        decimals: 1,
    },
    Unit {
        suffix: "_s",
        symbol: "s",
        decimals: 0,
    },
];

/// Metrics shown first, in this order; the rest follow alphabetically.
const PREFERRED: &[&str] = &["temperature", "humidity", "pressure", "co2"];

fn unit(name: &str) -> Option<&'static Unit> {
    UNITS
        .iter()
        .find(|unit| name.len() > unit.suffix.len() && name.ends_with(unit.suffix))
}

fn base(name: &str) -> &str {
    unit(name).map_or(name, |unit| &name[..name.len() - unit.suffix.len()])
}

/// "temperature_c" → "Temperature", "soil_moisture_pct" → "Soil moisture".
pub fn label(name: &str) -> String {
    let words = base(name).replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn symbol(name: &str) -> &'static str {
    unit(name).map_or("", |unit| unit.symbol)
}

/// The value without its unit, at the precision that fits the unit.
pub fn number(name: &str, value: f64) -> String {
    let decimals = decimals(name);
    let text = format!("{value:.decimals$}");
    // Avoid "-0" and "-0.0".
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

/// "26.2 °C", "52 %".
pub fn format(name: &str, value: f64) -> String {
    match symbol(name) {
        "" => number(name, value),
        symbol => format!("{} {symbol}", number(name, value)),
    }
}

/// Colour family of a metric: "temperature", "humidity", "pressure", "air",
/// "light", "power", "signal" or "other".
pub fn kind(name: &str) -> &'static str {
    match base(name) {
        "temperature" => return "temperature",
        "humidity" | "soil_moisture" => return "humidity",
        "pressure" => return "pressure",
        "co2" | "voc" | "pm25" | "pm10" => return "air",
        _ => {}
    }
    match unit(name).map(|unit| unit.suffix) {
        Some("_c") => "temperature",
        Some("_pct") => "humidity",
        Some("_hpa") => "pressure",
        Some("_ppm") => "air",
        Some("_lux") => "light",
        Some("_v" | "_a" | "_w" | "_mv") => "power",
        Some("_dbm") => "signal",
        _ => "other",
    }
}

pub fn decimals(name: &str) -> usize {
    unit(name).map_or(1, |unit| unit.decimals)
}

pub fn sort_metrics<T>(metrics: &mut [(String, T)]) {
    metrics.sort_by(|(a, _), (b, _)| {
        let rank = |name: &str| {
            PREFERRED
                .iter()
                .position(|preferred| base(name) == *preferred)
                .unwrap_or(PREFERRED.len())
        };
        rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_units() {
        assert_eq!(label("temperature_c"), "Temperature");
        assert_eq!(label("soil_moisture_pct"), "Soil moisture");
        assert_eq!(label("count"), "Count");
        assert_eq!(format("temperature_c", 26.234), "26.2 °C");
        assert_eq!(format("humidity_pct", 52.5), "52 %");
        assert_eq!(format("temperature_c", -0.04), "0.0 °C");
        assert_eq!(format("count", 3.0), "3.0");
        // A bare suffix is a name, not a unit.
        assert_eq!(symbol("_c"), "");
    }

    #[test]
    fn preferred_order() {
        let mut metrics = vec![
            ("battery_v".to_string(), ()),
            ("humidity_pct".to_string(), ()),
            ("temperature_c".to_string(), ()),
        ];
        sort_metrics(&mut metrics);
        let names: Vec<_> = metrics.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["temperature_c", "humidity_pct", "battery_v"]);
    }
}
