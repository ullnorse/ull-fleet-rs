//! Activity: every event in the fleet, live, with filters.

use leptos::prelude::*;
use ull_fleet_types::EventView;

use crate::api;
use crate::components::events::{EventList, category};
use crate::components::shell::PageHeader;
use crate::store::use_fleet;

const FILTERS: [(&str, &str); 4] = [
    ("all", "All"),
    ("alerts", "Alerts"),
    ("updates", "Updates"),
    ("connectivity", "Connectivity"),
];

#[component]
pub fn ActivityPage() -> impl IntoView {
    let fleet = use_fleet();
    let filter = RwSignal::new("all");
    let older = RwSignal::new(Vec::<EventView>::new());
    let exhausted = RwSignal::new(false);
    let loading = RwSignal::new(false);

    let events = Signal::derive(move || {
        let filter = filter.get();
        let mut all: Vec<EventView> = fleet.events.get();
        older.with(|o| all.extend(o.iter().cloned()));
        all.sort_by_key(|e| std::cmp::Reverse(e.id));
        all.dedup_by_key(|e| e.id);
        all.retain(|e| filter == "all" || category(&e.kind) == filter);
        all
    });
    let load_older = move |_| {
        let before = events.with_untracked(|e| e.last().map(|e| e.id));
        loading.set(true);
        leptos::task::spawn_local(async move {
            let mut url = "/api/v1/events?limit=100".to_string();
            if let Some(before) = before {
                url.push_str(&format!("&before={before}"));
            }
            match api::get::<Vec<EventView>>(&url).await {
                Ok(list) => {
                    exhausted.set(list.len() < 100);
                    older.update(|o| o.extend(list));
                }
                Err(err) => fleet.error(&err),
            }
            loading.set(false);
        });
    };

    view! {
        <PageHeader title="Activity" />
        <div class="page">
            <div class="feed">
                <div class="chips" role="tablist">
                    {FILTERS
                        .into_iter()
                        .map(|(key, label)| {
                            view! {
                                <button
                                    class="chip"
                                    role="tab"
                                    aria-selected=move || (filter.get() == key).to_string()
                                    class:on=move || filter.get() == key
                                    on:click=move |_| filter.set(key)
                                >
                                    {label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="card">
                    <Show
                        when=move || events.with(|e| !e.is_empty())
                        fallback=|| view! { <p class="muted">"Nothing here yet."</p> }
                    >
                        <EventList events=events show_device=true />
                    </Show>
                    <Show when=move || !exhausted.get()>
                        <button class="btn ghost small wide" disabled=move || loading.get() on:click=load_older>
                            "Older"
                        </button>
                    </Show>
                </div>
            </div>
        </div>
    }
}
