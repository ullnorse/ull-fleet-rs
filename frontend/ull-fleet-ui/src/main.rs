//! ull-fleet web UI: a Leptos single-page app fed by the server's JSON API
//! and its Server-Sent Events stream.

mod api;
mod components;
mod format;
mod icons;
mod pages;
mod store;

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use components::shell::Shell;
use pages::{
    activity::ActivityPage, device::DevicePage, login::LoginPage, overview::Overview,
    releases::ReleasesPage, settings::SettingsPage, trends::TrendsPage,
};
use store::{Auth, Fleet};

fn main() {
    console_error_panic_hook::set_once();
    if let Some(boot) = document().get_element_by_id("boot") {
        boot.remove();
    }
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let fleet = Fleet::new();
    provide_context(fleet);
    fleet.start();

    view! {
        <Router>
            {move || match fleet.auth.get() {
                Auth::SignInNeeded => view! { <LoginPage /> }.into_any(),
                _ => {
                    view! {
                        <Shell>
                            <Routes fallback=|| view! { <p class="empty">"Page not found."</p> }>
                                <Route path=path!("/") view=Overview />
                                <Route path=path!("/devices/:id") view=DevicePage />
                                <Route path=path!("/trends") view=TrendsPage />
                                <Route path=path!("/activity") view=ActivityPage />
                                <Route path=path!("/releases") view=ReleasesPage />
                                <Route path=path!("/settings") view=SettingsPage />
                            </Routes>
                        </Shell>
                    }
                        .into_any()
                }
            }}
        </Router>
    }
}
