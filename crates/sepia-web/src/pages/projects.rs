//! `/projects` — named groups over session meta. Lists are merged
//! across nodes on a hub (each row carries a `node` badge); creates go
//! to the primary node, deletes to the owning one.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::api::{create_project, delete_project, list_projects, list_sessions};
use crate::dto::{ProjectDto, SessionSummaryDto};

#[component]
pub fn ProjectsPage() -> impl IntoView {
    let projects = Resource::new(|| (), |()| list_projects());
    let sessions = Resource::new(|| (), |()| list_sessions());

    let draft = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let form_error: RwSignal<Option<String>> = RwSignal::new(None);

    let create = move || {
        let name = draft.get();
        if name.trim().is_empty() || busy.get() {
            return;
        }
        busy.set(true);
        form_error.set(None);
        leptos::task::spawn_local(async move {
            match create_project(name, None).await {
                Ok(_) => {
                    draft.set(String::new());
                    projects.refetch();
                }
                Err(e) => form_error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    view! {
        <Title text="projects — sepia"/>
        <section class="page">
            <header class="page-head">
                <h1>"Projects"</h1>
            </header>
            <form
                class="form-row"
                on:submit=move |ev| {
                    ev.prevent_default();
                    create();
                }
            >
                <input
                    class="field"
                    type="text"
                    placeholder="New project name…"
                    maxlength=100
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                />
                <button
                    class="send"
                    type="submit"
                    disabled=move || busy.get() || draft.read().trim().is_empty()
                >
                    {move || if busy.get() { "Creating…" } else { "Create" }}
                </button>
            </form>
            {move || form_error.get().map(|e| view! { <p class="error">{e}</p> })}
            <Suspense fallback=move || {
                view! { <p class="loading">"Loading projects…"</p> }
            }>
                {move || {
                    Suspend::new(async move {
                        let sessions_list = sessions.await.unwrap_or_default();
                        match projects.await {
                            Err(e) => {
                                view! { <p class="error">{e.to_string()}</p> }.into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! { <p class="empty">"No projects yet."</p> }.into_any()
                            }
                            Ok(list) => {
                                view! {
                                    <ul class="card-list">
                                        {list
                                            .into_iter()
                                            .map(|p| {
                                                let n = session_count(&sessions_list, &p);
                                                view! {
                                                    <ProjectRow
                                                        project=p
                                                        count=n
                                                        on_deleted=move || projects.refetch()
                                                    />
                                                }
                                            })
                                            .collect::<Vec<_>>()}
                                    </ul>
                                }
                                    .into_any()
                            }
                        }
                    })
                }}
            </Suspense>
        </section>
    }
}

/// Sessions whose `projectIds` mention this project. Node ids disam-
/// biguate when the merged list carries them (a bare `project_id` could
/// collide across nodes).
fn session_count(sessions: &[SessionSummaryDto], project: &ProjectDto) -> usize {
    sessions
        .iter()
        .filter(|s| {
            s.project_ids.contains(&project.id)
                && (project.node.is_none() || s.node == project.node)
        })
        .count()
}

#[component]
fn ProjectRow(
    project: ProjectDto,
    count: usize,
    on_deleted: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    let deleting = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let id = project.id.clone();
    let node = project.node.clone();
    let on_delete = move |_| {
        if deleting.get() {
            return;
        }
        deleting.set(true);
        error.set(None);
        let id = id.clone();
        let node = node.clone();
        leptos::task::spawn_local(async move {
            match delete_project(id, node).await {
                Ok(()) => on_deleted(),
                Err(e) => {
                    error.set(Some(e.to_string()));
                    deleting.set(false);
                }
            }
        });
    };
    view! {
        <li class="card">
            <div class="card-head">
                <span class="card-title">{project.name.clone()}</span>
                <span class="badges">
                    {project.node.clone().map(|n| view! { <span class="badge node">{n}</span> })}
                    <span class="badge">{format!("{count} sessions")}</span>
                </span>
            </div>
            <p class="card-meta">
                <code>{project.id.clone()}</code>
            </p>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            <div class="card-actions">
                <button
                    class="danger"
                    disabled=move || deleting.get()
                    on:click=on_delete
                >
                    {move || if deleting.get() { "Deleting…" } else { "Delete" }}
                </button>
            </div>
        </li>
    }
}
