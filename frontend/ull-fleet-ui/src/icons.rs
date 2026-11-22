//! Inline SVG icons (24 px grid, 2 px strokes).

use leptos::prelude::*;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Icon {
    Devices,
    Activity,
    Releases,
    Settings,
    Back,
    Alert,
    Check,
    Download,
    Upload,
    Trash,
    Edit,
    Close,
    Plus,
    Power,
    WifiOff,
    Bell,
    Cpu,
    Rocket,
}

fn paths(icon: Icon) -> &'static str {
    match icon {
        Icon::Devices => "M4 5h16v10H4z M9 19h6 M12 15v4",
        Icon::Activity => "M3 12h4l3-8 4 16 3-8h4",
        Icon::Releases => "M12 3l8 4.5v9L12 21l-8-4.5v-9z M12 12l8-4.5 M12 12v9 M12 12L4 7.5",
        Icon::Settings => "M4 6h10 M18 6h2 M4 12h4 M12 12h8 M4 18h12 M20 18h0 M16 4v4 M10 10v4 M18 16v4",
        Icon::Back => "M15 5l-7 7 7 7",
        Icon::Alert => "M12 4l9 16H3z M12 10v4 M12 17.5v.01",
        Icon::Check => "M5 12.5l4.5 4.5L19 7.5",
        Icon::Download => "M12 4v11 M7 10l5 5 5-5 M5 20h14",
        Icon::Upload => "M12 20V9 M7 14l5-5 5 5 M5 4h14",
        Icon::Trash => "M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13",
        Icon::Edit => "M4 20h4L19 9l-4-4L4 16z M13 7l4 4",
        Icon::Close => "M6 6l12 12 M18 6L6 18",
        Icon::Plus => "M12 5v14 M5 12h14",
        Icon::Power => "M12 3v8 M6.3 7a8 8 0 1 0 11.4 0",
        Icon::WifiOff => "M3 3l18 18 M8.5 16.5a5 5 0 0 1 7 0 M5 12.5a10 10 0 0 1 5-2.6 M19 12.5a10 10 0 0 0-2.3-1.6 M2 9a15 15 0 0 1 4.2-2.6 M22 9a15 15 0 0 0-10-3.9 M12 20h.01",
        Icon::Bell => "M6 16V11a6 6 0 0 1 12 0v5l2 2H4z M10 21h4",
        Icon::Cpu => "M7 7h10v10H7z M10 3v4 M14 3v4 M10 17v4 M14 17v4 M3 10h4 M3 14h4 M17 10h4 M17 14h4",
        Icon::Rocket => "M12 15l-3-3c1.5-5 5-8 10-8 0 5-3 8.5-8 10z M9 12l-4 1 2-4h4 M12 15l-1 4 4-2v-4 M5 19l2-2",
    }
}

#[component]
pub fn Svg(icon: Icon, #[prop(optional)] class: &'static str) -> impl IntoView {
    view! {
        <svg class=format!("icon {class}") viewBox="0 0 24 24" aria-hidden="true">
            <path d=paths(icon) />
        </svg>
    }
}

/// Wi-Fi bars, filled up to `bars` of 4.
#[component]
pub fn SignalBars(#[prop(into)] bars: Signal<u8>) -> impl IntoView {
    view! {
        <svg class="signal" viewBox="0 0 20 16" aria-hidden="true">
            {(1..=4u8)
                .map(|n| {
                    let h = 3.0 + f64::from(n) * 3.0;
                    view! {
                        <rect
                            x={f64::from(n - 1) * 5.0}
                            y={16.0 - h}
                            width="3.2"
                            height=h
                            rx="1"
                            class:on={move || bars.get() >= n}
                        />
                    }
                })
                .collect_view()}
        </svg>
    }
}
