//! Releases: signed images per project, how far each has rolled out, and
//! uploading new ones.

use leptos::prelude::*;
use ull_fleet_types::{DeployReply, DeployRequest, ReleaseView};
use wasm_bindgen_futures::JsFuture;

use crate::api;
use crate::components::shell::PageHeader;
use crate::components::widgets::ConfirmButton;
use crate::format;
use crate::icons::{Icon, Svg};
use crate::store::{Tone, use_fleet};

#[component]
pub fn ReleasesPage() -> impl IntoView {
    let fleet = use_fleet();
    let uploading = RwSignal::new(false);
    let projects = Memo::new(move |_| {
        let mut projects: Vec<String> = fleet
            .releases
            .with(|r| r.iter().map(|r| r.project.clone()).collect());
        projects.dedup();
        projects
    });

    view! {
        <PageHeader title="Releases">
            <button class="btn primary small" on:click=move |_| uploading.update(|u| *u = !*u)>
                <Svg icon=Icon::Upload />
                <span>"Upload"</span>
            </button>
        </PageHeader>
        <div class="page">
            <div class="releases">
                <Show when=move || uploading.get()>
                    <UploadCard open=uploading />
                </Show>
                <Show
                    when=move || projects.with(|p| !p.is_empty())
                    fallback=|| view! {
                        <div class="empty-state">
                            <Svg icon=Icon::Releases class="empty-icon" />
                            <h2>"No releases yet"</h2>
                            <p>"Upload a signed image, or run " <code>"scripts/publish.sh <firmware.elf>"</code> " from ull-fleet-rs."</p>
                        </div>
                    }
                >
                    <For each=move || projects.get() key=|p| p.clone() let:project>
                        <ProjectCard project=project />
                    </For>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn ProjectCard(project: String) -> impl IntoView {
    let fleet = use_fleet();
    let name = StoredValue::new(project.clone());
    let releases = Memo::new(move |_| {
        name.with_value(|p| {
            fleet
                .releases
                .with(|r| r.iter().filter(|r| r.project == *p).cloned().collect::<Vec<_>>())
        })
    });
    let device_count = Memo::new(move |_| {
        name.with_value(|p| {
            fleet
                .all_devices()
                .iter()
                .filter(|d| d.with(|d| d.project == *p))
                .count() as i64
        })
    });
    view! {
        <section class="card">
            <div class="project-head">
                <h2>{project}</h2>
                <span class="muted">{move || match device_count.get() {
                    1 => "1 device".to_string(),
                    n => format!("{n} devices"),
                }}</span>
            </div>
            <For
                each={move || releases.get().into_iter().enumerate().collect::<Vec<_>>()}
                key=|(i, r)| (*i == 0, r.id, r.running, r.updating, r.notes.clone())
                let:item
            >
                <ReleaseRow release=item.1 latest={item.0 == 0} devices=device_count />
            </For>
        </section>
    }
}

#[component]
fn ReleaseRow(release: ReleaseView, latest: bool, devices: Memo<i64>) -> impl IntoView {
    let fleet = use_fleet();
    let id = release.id;
    let version = format::version(&release.version);
    let base = version.base.to_string();
    let rev = version.rev.map(str::to_string);
    let built = version.built.map(format::day_clock);
    let running = release.running;
    let updating = release.updating;
    let deploy = move || {
        leptos::task::spawn_local(async move {
            match api::send::<_, DeployReply>(
                "POST",
                &format!("/api/v1/releases/{id}/deploy"),
                &DeployRequest { devices: None },
            )
            .await
            {
                Ok(reply) => {
                    let body = match (reply.targeted.len(), reply.skipped.len()) {
                        (0, 0) => "Every device already runs it.".to_string(),
                        (n, 0) => format!("{n} device{} will update at the next check-in.", if n == 1 { "" } else { "s" }),
                        (n, s) => format!("{n} queued; {s} skipped: {}", reply.skipped[0].reason),
                    };
                    fleet.toast(Tone::Info, "Rollout started".into(), body, None);
                }
                Err(err) => fleet.error(&err),
            }
        });
    };
    let delete = move || {
        leptos::task::spawn_local(async move {
            if let Err(err) = api::send_empty::<()>("DELETE", &format!("/api/v1/releases/{id}"), None).await {
                fleet.error(&err);
            }
        });
    };
    let total = move || devices.get().max(running + updating);
    view! {
        <article class="release">
            <div class="release-main">
                <div class="release-version">
                    <span>{base}</span>
                    {rev.map(|rev| view! { <code class="rev">{rev}</code> })}
                    {latest.then(|| view! { <span class="badge latest">"Latest"</span> })}
                </div>
                <span class="release-meta">
                    {[built.map(|b| format!("Built {b}")), Some(format::bytes(release.size)), Some(format!("uploaded {}", format::day_clock(release.created_at)))]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ")}
                </span>
                {(!release.notes.is_empty()).then(|| view! { <p class="release-notes">{release.notes.clone()}</p> })}
            </div>
            <div class="release-actions">
                <Show when=move || { running + updating < devices.get() }>
                    <ConfirmButton
                        label="Install on all"
                        confirm="Tap to confirm"
                        class="primary small"
                        on_confirm=deploy
                    />
                </Show>
                <Show when=move || { running == 0 && updating == 0 }>
                    <ConfirmButton label="" confirm="Delete" class="ghost small icon-only" icon=Icon::Trash on_confirm=delete />
                </Show>
            </div>
            <Show when=move || { total() > 0 }>
                <div class="coverage">
                    <div class="coverage-text">
                        <span>{move || format!("Running on {running} of {}", total())}</span>
                        {(updating > 0).then(|| view! { <span class="foot-text busy">{format!("{updating} updating")}</span> })}
                    </div>
                    <div class="bar">
                        <div
                            class:busy={updating > 0}
                            style=move || format!("width:{:.1}%", (running + updating) as f64 / total().max(1) as f64 * 100.0)
                        ></div>
                    </div>
                </div>
            </Show>
        </article>
    }
}

/// Reads a small text file (a saved signature).
async fn read_text(file: &web_sys::File) -> Option<String> {
    let text = JsFuture::from(file.text()).await.ok()?;
    text.as_string()
}

#[component]
fn UploadCard(open: RwSignal<bool>) -> impl IntoView {
    let fleet = use_fleet();
    let image = RwSignal::new_local(None::<web_sys::File>);
    let signature = RwSignal::new(String::new());
    let notes = RwSignal::new(String::new());
    let over = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    let take_files = move |files: web_sys::FileList| {
        for i in 0..files.length() {
            let Some(file) = files.item(i) else { continue };
            if file.size() <= 1024.0 && !file.name().ends_with(".bin") {
                leptos::task::spawn_local(async move {
                    if let Some(text) = read_text(&file).await {
                        signature.set(text.trim().to_string());
                    }
                });
            } else {
                image.set(Some(file));
            }
        }
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(file) = image.get_untracked() else {
            error.set(Some("Choose an image file.".into()));
            return;
        };
        busy.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::upload_release(&file, &signature.get_untracked(), &notes.get_untracked()).await {
                Ok(release) => {
                    fleet.toast(
                        Tone::Good,
                        "Release uploaded".into(),
                        format!("{} {}", release.project, format::short_version(&release.version)),
                        None,
                    );
                    open.set(false);
                }
                Err(err) => error.set(Some(err.to_string())),
            }
            busy.set(false);
        });
    };

    view! {
        <form class="card upload" on:submit=submit>
            <h2 class="card-title"><Svg icon=Icon::Upload />"Upload a release"</h2>
            <label
                class="drop"
                class:over=move || over.get()
                class:has-file=move || image.with(Option::is_some)
                on:dragover=move |ev| { ev.prevent_default(); over.set(true); }
                on:dragleave=move |_| over.set(false)
                on:drop=move |ev: leptos::ev::DragEvent| {
                    ev.prevent_default();
                    over.set(false);
                    if let Some(files) = ev.data_transfer().and_then(|d| d.files()) {
                        take_files(files);
                    }
                }
            >
                <Svg icon=Icon::Upload />
                {move || match image.get() {
                    Some(file) => view! {
                        <strong>{file.name()}</strong>
                        <span class="hint">{format::bytes(file.size() as i64)}</span>
                    }.into_any(),
                    None => view! {
                        <strong>"Choose an image"</strong>
                        <span class="hint">"Application image from " <code>"espflash save-image"</code> ". Drop its signature file here too."</span>
                    }.into_any(),
                }}
                <input
                    type="file"
                    multiple
                    node_ref=input_ref
                    on:change=move |_| {
                        if let Some(files) = input_ref.get_untracked().and_then(|i| i.files()) {
                            take_files(files);
                        }
                    }
                />
            </label>
            <label class="field">
                <span>"Signature"</span>
                <input
                    type="text"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="Output of ull-fleet-sign sign <key> <image>"
                    prop:value=move || signature.get()
                    on:input=move |ev| signature.set(event_target_value(&ev))
                />
            </label>
            <label class="field">
                <span>"Notes"</span>
                <input
                    type="text"
                    maxlength="500"
                    placeholder="What changed"
                    prop:value=move || notes.get()
                    on:input=move |ev| notes.set(event_target_value(&ev))
                />
            </label>
            {move || error.get().map(|e| view! { <p class="error-text">{e}</p> })}
            <div class="form-actions">
                <button type="button" class="btn ghost" on:click=move |_| open.set(false)>"Cancel"</button>
                <button type="submit" class="btn primary" disabled=move || busy.get()>
                    {move || if busy.get() { "Uploading" } else { "Upload" }}
                </button>
            </div>
            <p class="hint">"From a terminal: " <code>"scripts/publish.sh <firmware.elf> [notes]"</code> " builds, signs and uploads in one step."</p>
        </form>
    }
}
