//! Settings: appearance and what the server is configured with.

use leptos::prelude::*;

use crate::api;
use crate::components::shell::PageHeader;
use crate::components::widgets::{Row, Segmented};
use crate::format;
use crate::store::use_fleet;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Theme {
    System,
    Light,
    Dark,
}

fn stored_theme() -> Theme {
    match window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item("theme").ok().flatten())
        .as_deref()
    {
        Some("light") => Theme::Light,
        Some("dark") => Theme::Dark,
        _ => Theme::System,
    }
}

fn apply_theme(theme: Theme) {
    let root = document().document_element();
    let storage = window().local_storage().ok().flatten();
    let value = match theme {
        Theme::System => None,
        Theme::Light => Some("light"),
        Theme::Dark => Some("dark"),
    };
    if let Some(root) = root {
        match value {
            Some(v) => {
                let _ = root.set_attribute("data-theme", v);
            }
            None => {
                let _ = root.remove_attribute("data-theme");
            }
        }
    }
    if let Some(storage) = storage {
        let _ = match value {
            Some(v) => storage.set_item("theme", v),
            None => storage.remove_item("theme"),
        };
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let fleet = use_fleet();
    let theme = RwSignal::new(stored_theme());
    Effect::new(move |_| apply_theme(theme.get()));
    let server = move || fleet.server.get();
    let sign_out = move |_| {
        leptos::task::spawn_local(async move {
            let _ = api::send_empty::<()>("POST", "/api/v1/logout", None).await;
            fleet.check_session().await;
        });
    };

    view! {
        <PageHeader title="Settings" />
        <div class="page">
            <div class="settings">
                <section class="card">
                    <h2 class="card-title">"Appearance"</h2>
                    <Segmented
                        label="Theme"
                        value=theme
                        options=vec![(Theme::System, "System"), (Theme::Light, "Light"), (Theme::Dark, "Dark")]
                    />
                </section>
                {move || server().map(|s| view! {
                    <section class="card">
                        <h2 class="card-title">"Devices"</h2>
                        <dl class="rows">
                            <Row label="Check-in interval">{format!("{} s", s.poll_interval_s)}</Row>
                            <Row label="While a device page is open">{format!("{} s", s.live_poll_interval_s)}</Row>
                            <Row label="Offline after">{format!("{} s without a check-in", (s.poll_interval_s * 3).max(60))}</Row>
                            <Row label="Readings kept">{format!("{} days", s.retention_days)}</Row>
                        </dl>
                    </section>
                    <section class="card">
                        <h2 class="card-title">"Security"</h2>
                        <dl class="rows">
                            <Row label="Login">{if s.auth_required { "Password required" } else { "Off" }}</Row>
                            <Row label="Release signing key">
                                {match &s.signing_key {
                                    Some(key) => view! { <code>{format!("{}…{}", &key[..8], &key[key.len() - 8..])}</code> }.into_any(),
                                    None => view! { <span>"Not set: uploads are not checked"</span> }.into_any(),
                                }}
                            </Row>
                        </dl>
                        {s.auth_required.then(|| view! {
                            <div class="form-actions"><button class="btn ghost small" on:click=sign_out>"Sign out"</button></div>
                        })}
                    </section>
                    <section class="card">
                        <h2 class="card-title">"Server"</h2>
                        <dl class="rows">
                            <Row label="Version">{s.version.clone()}</Row>
                            <Row label="Server time">{move || format::day_clock(fleet.now.get())}</Row>
                        </dl>
                    </section>
                })}
            </div>
        </div>
    }
}
