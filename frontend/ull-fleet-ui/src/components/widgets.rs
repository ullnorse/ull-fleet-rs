//! Small shared pieces.

use leptos::prelude::*;
use ull_fleet_types::{DeviceView, UpdateState};

use crate::icons::{Icon, Svg};

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Status {
    Online,
    Offline,
    Updating,
    Alert,
    Failed,
}

impl Status {
    pub fn of(device: &DeviceView) -> Self {
        if !device.online {
            Status::Offline
        } else if !device.alerts.is_empty() {
            Status::Alert
        } else if matches!(
            device.update.state,
            UpdateState::Pending | UpdateState::Downloading
        ) {
            Status::Updating
        } else if device.update.state == UpdateState::Failed {
            Status::Failed
        } else {
            Status::Online
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Status::Online => "online",
            Status::Offline => "offline",
            Status::Updating => "updating",
            Status::Alert => "alert",
            Status::Failed => "failed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Online => "Online",
            Status::Offline => "Offline",
            Status::Updating => "Updating",
            Status::Alert => "Alert",
            Status::Failed => "Update failed",
        }
    }
}

/// A button that asks for a second tap within 4 s before acting.
#[component]
pub fn ConfirmButton(
    label: &'static str,
    confirm: &'static str,
    #[prop(optional)] class: &'static str,
    #[prop(optional)] icon: Option<Icon>,
    #[prop(into, optional)] disabled: Signal<bool>,
    on_confirm: impl Fn() + 'static,
) -> impl IntoView {
    let armed = RwSignal::new(false);
    view! {
        <button
            class=format!("btn {class}")
            class:armed=move || armed.get()
            disabled=move || disabled.get()
            on:click=move |_| {
                if armed.get_untracked() {
                    armed.set(false);
                    on_confirm();
                } else {
                    armed.set(true);
                    set_timeout(move || armed.set(false), std::time::Duration::from_secs(4));
                }
            }
        >
            {icon.map(|icon| view! { <Svg icon=icon /> })}
            <span>{move || if armed.get() { confirm } else { label }}</span>
        </button>
    }
}

/// Segmented control.
#[component]
pub fn Segmented<T>(
    options: Vec<(T, &'static str)>,
    value: RwSignal<T>,
    #[prop(optional)] label: &'static str,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    view! {
        <div class="segmented" role="radiogroup" aria-label=label>
            {options
                .into_iter()
                .map(|(option, text)| {
                    view! {
                        <button
                            role="radio"
                            aria-checked=move || (value.get() == option).to_string()
                            class:on=move || value.get() == option
                            on:click=move |_| value.set(option)
                        >
                            {text}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// A label/value row in a details list.
#[component]
pub fn Row(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <div class="row">
            <dt>{label}</dt>
            <dd>{children()}</dd>
        </div>
    }
}
