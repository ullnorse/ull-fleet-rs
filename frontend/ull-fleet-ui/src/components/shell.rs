//! App frame: sidebar on wide screens, tab bar on phones, connection state
//! and toasts.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::icons::{Icon, Svg};
use crate::store::{Connection, Tone, use_fleet};

const NAV: [(&str, &str, Icon); 5] = [
    ("/", "Devices", Icon::Devices),
    ("/trends", "Trends", Icon::Trends),
    ("/activity", "Activity", Icon::Activity),
    ("/releases", "Releases", Icon::Releases),
    ("/settings", "Settings", Icon::Settings),
];

#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let fleet = use_fleet();
    let location = use_location();
    let active = move |href: &str| {
        let path = location.pathname.get();
        if href == "/" {
            path == "/" || path.starts_with("/devices")
        } else {
            path.starts_with(href)
        }
    };
    let alerts = Memo::new(move |_| {
        fleet
            .all_devices()
            .iter()
            .filter(|d| d.with(|d| !d.alerts.is_empty()))
            .count()
    });

    view! {
        <div class="app">
            <nav class="sidebar" aria-label="Main">
                <div class="brand">
                    <span class="brand-mark"></span>
                    <span>"ull-fleet"</span>
                </div>
                {NAV
                    .into_iter()
                    .map(|(href, label, icon)| {
                        view! {
                            <A href=href attr:class="nav-item" attr:data-active=move || active(href).to_string()>
                                <Svg icon=icon />
                                <span>{label}</span>
                                {(href == "/")
                                    .then(|| {
                                        view! {
                                            <Show when=move || { alerts.get() > 0 }>
                                                <span class="nav-badge">{move || alerts.get()}</span>
                                            </Show>
                                        }
                                    })}
                            </A>
                        }
                    })
                    .collect_view()}
                <div class="sidebar-foot">
                    <ConnectionState />
                </div>
            </nav>
            <main class="main">{children()}</main>
            <nav class="tabbar" aria-label="Main">
                {NAV
                    .into_iter()
                    .map(|(href, label, icon)| {
                        view! {
                            <A href=href attr:class="tab" attr:data-active=move || active(href).to_string()>
                                <span class="tab-icon">
                                    <Svg icon=icon />
                                    {(href == "/")
                                        .then(|| {
                                            view! {
                                                <Show when=move || { alerts.get() > 0 }>
                                                    <span class="tab-badge"></span>
                                                </Show>
                                            }
                                        })}
                                </span>
                                <span>{label}</span>
                            </A>
                        }
                    })
                    .collect_view()}
            </nav>
            <Toasts />
        </div>
    }
}

/// "Live" with a pulsing dot, or "Reconnecting".
#[component]
pub fn ConnectionState() -> impl IntoView {
    let fleet = use_fleet();
    view! {
        <span class="conn" data-state=move || match fleet.connection.get() {
            Connection::Live => "live",
            Connection::Connecting => "connecting",
            Connection::Reconnecting => "reconnecting",
        }>
            <span class="conn-dot"></span>
            {move || match fleet.connection.get() {
                Connection::Live => "Live",
                Connection::Connecting => "Connecting",
                Connection::Reconnecting => "Reconnecting",
            }}
        </span>
    }
}

/// Page header: title, optional back link, and the connection state on phones.
#[component]
pub fn PageHeader(
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] back: Option<&'static str>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="page-header">
            {back.map(|href| view! {
                <A href=href attr:class="back" attr:aria-label="Back">
                    <Svg icon=Icon::Back />
                </A>
            })}
            <h1>{move || title.get()}</h1>
            <div class="page-header-end">
                {children.map(|c| c())}
                <span class="phone-only"><ConnectionState /></span>
            </div>
        </header>
    }
}

#[component]
fn Toasts() -> impl IntoView {
    let fleet = use_fleet();
    view! {
        <div class="toasts" role="status" aria-live="polite">
            <For each=move || fleet.toasts.get() key=|toast| toast.id let:toast>
                {
                    let id = toast.id;
                    let tone = match toast.tone {
                        Tone::Good => "good",
                        Tone::Warning => "warning",
                        Tone::Critical => "critical",
                        Tone::Info => "info",
                    };
                    let icon = match toast.tone {
                        Tone::Good => Icon::Check,
                        Tone::Warning => Icon::WifiOff,
                        Tone::Critical => Icon::Alert,
                        Tone::Info => Icon::Plus,
                    };
                    let href = toast.device_id.map(|d| format!("/devices/{d}"));
                    view! {
                        <div class="toast" data-tone=tone>
                            <span class="toast-icon"><Svg icon=icon /></span>
                            {match href {
                                Some(href) => view! {
                                    <A href=href attr:class="toast-text">
                                        <strong>{toast.title}</strong>
                                        <span>{toast.body}</span>
                                    </A>
                                }.into_any(),
                                None => view! {
                                    <div class="toast-text">
                                        <strong>{toast.title}</strong>
                                        <span>{toast.body}</span>
                                    </div>
                                }.into_any(),
                            }}
                            <button
                                class="toast-close"
                                aria-label="Dismiss"
                                on:click=move |_| fleet.toasts.update(|t| t.retain(|x| x.id != id))
                            >
                                <Svg icon=Icon::Close />
                            </button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}
