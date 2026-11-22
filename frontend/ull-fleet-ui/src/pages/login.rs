//! Sign-in screen, shown when the server has a password and there is no session.

use leptos::prelude::*;
use ull_fleet_types::LoginRequest;

use crate::api;
use crate::store::use_fleet;

#[component]
pub fn LoginPage() -> impl IntoView {
    let fleet = use_fleet();
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        busy.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let body = LoginRequest { password: password.get_untracked() };
            match api::send_empty("POST", "/api/v1/login", Some(&body)).await {
                Ok(()) => fleet.check_session().await,
                Err(api::Error::Unauthorized) => error.set(Some("Wrong password.".into())),
                Err(err) => error.set(Some(err.to_string())),
            }
            busy.set(false);
        });
    };
    view! {
        <div class="login">
            <form class="card login-card" on:submit=submit>
                <span class="brand-mark"></span>
                <h1>"Sign in to ull-fleet"</h1>
                <label class="field">
                    <span>"Password"</span>
                    <input
                        type="password"
                        autocomplete="current-password"
                        autofocus
                        prop:value=move || password.get()
                        on:input=move |ev| password.set(event_target_value(&ev))
                    />
                </label>
                {move || error.get().map(|e| view! { <p class="error-text">{e}</p> })}
                <button class="btn primary" type="submit" disabled=move || busy.get()>"Sign in"</button>
            </form>
        </div>
    }
}
