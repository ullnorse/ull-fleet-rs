//! Event rows for the activity views.

use leptos::prelude::*;
use leptos_router::components::A;
use ull_fleet_types::EventView;

use crate::format;
use crate::icons::{Icon, Svg};
use crate::store::use_fleet;

pub fn tone(kind: &str) -> &'static str {
    match kind {
        "alert" | "update_failed" => "critical",
        "offline" => "warning",
        "update" | "alert_cleared" | "online" => "good",
        _ => "neutral",
    }
}

fn icon(kind: &str) -> Icon {
    match kind {
        "alert" => Icon::Alert,
        "alert_cleared" => Icon::Check,
        "update_failed" => Icon::Alert,
        "update" => Icon::Check,
        "deploy" => Icon::Rocket,
        "firmware" => Icon::Cpu,
        "offline" => Icon::WifiOff,
        "online" => Icon::Activity,
        "restarted" => Icon::Power,
        "registered" => Icon::Plus,
        _ => Icon::Bell,
    }
}

/// Which filter chip an event falls under.
pub fn category(kind: &str) -> &'static str {
    match kind {
        "alert" | "alert_cleared" => "alerts",
        "update" | "update_failed" | "deploy" | "firmware" => "updates",
        "offline" | "online" | "restarted" | "registered" => "connectivity",
        _ => "other",
    }
}

#[component]
pub fn EventRow(event: EventView, #[prop(optional)] show_device: bool) -> impl IntoView {
    let fleet = use_fleet();
    let device_id = event.device_id.clone();
    view! {
        <li class="event" data-tone=tone(&event.kind)>
            <span class="event-icon"><Svg icon=icon(&event.kind) /></span>
            <div class="event-body">
                <span class="event-message">{format::tidy_message(&event.message)}</span>
                {show_device.then(|| {
                    let href = format!("/devices/{device_id}");
                    let id = device_id.clone();
                    view! {
                        <A href=href attr:class="event-device">
                            {move || { fleet.ids.track(); fleet.device_name(&id) }}
                        </A>
                    }
                })}
            </div>
            <time class="event-time">{format::clock(event.ts)}</time>
        </li>
    }
}

/// Events grouped under day headings, newest first.
#[component]
pub fn EventList(
    #[prop(into)] events: Signal<Vec<EventView>>,
    #[prop(optional)] show_device: bool,
) -> impl IntoView {
    let fleet = use_fleet();
    let groups = Memo::new(move |_| {
        let now = fleet.now.get() / 60 * 60;
        let mut groups: Vec<(String, Vec<EventView>)> = Vec::new();
        for event in events.get() {
            let heading = format::day_heading(event.ts, now);
            match groups.last_mut() {
                Some((h, list)) if *h == heading => list.push(event),
                _ => groups.push((heading, vec![event])),
            }
        }
        groups
    });
    view! {
        <div class="event-groups">
            <For each=move || groups.with(|g| g.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>()) key=|heading| heading.clone() let:heading>
                <section class="event-group">
                    <h3 class="group-heading">{heading.clone()}</h3>
                    <ul class="events">
                        <For
                            each=move || {
                                groups.with(|g| {
                                    g.iter()
                                        .find(|(h, _)| *h == heading)
                                        .map(|(_, list)| list.clone())
                                        .unwrap_or_default()
                                })
                            }
                            key=|e| e.id
                            let:event
                        >
                            <EventRow event=event show_device=show_device />
                        </For>
                    </ul>
                </section>
            </For>
        </div>
    }
}
