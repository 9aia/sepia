//! `/projects` — named groups over session meta. Lists are merged
//! across nodes on a hub (each row carries a `node` badge); creates go
//! to the primary node, deletes to the owning one.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::api::{create_project, delete_project};
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardHeader,
    CardTitle, Input, PageDescription, PageHead, PageTitle, Skeleton,
};
use crate::dto::{ProjectDto, SessionSummaryDto};

#[component]
pub fn ProjectsPage() -> impl IntoView {
    let client = crate::api::query_client();
    let projects = client.resource(crate::api::projects_scope, || ());
    let sessions = client.resource(crate::api::sessions_scope, || ());

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
        <section class="mx-auto w-full max-w-5xl space-y-6 p-4 md:p-6">
            <PageHead class="mb-0">
                <div>
                    <PageTitle>"Projects"</PageTitle>
                    <PageDescription>"Named groups over session metadata."</PageDescription>
                </div>
            </PageHead>
            <form
                class="flex items-center gap-2"
                on:submit=move |ev| {
                    ev.prevent_default();
                    create();
                }
            >
                <Input
                    class="w-full max-w-xs"
                    attr:r#type="text"
                    attr:placeholder="New project name…"
                    attr:maxlength=100
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                />
                <Button
                    button_type="submit"
                    disabled=move || busy.get() || draft.read().trim().is_empty()
                >
                    {move || if busy.get() { "Creating…" } else { "Create" }}
                </Button>
            </form>
            {move || {
                form_error.get().map(|e| {
                    view! {
                        <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                            {e}
                        </p>
                    }
                })
            }}
            <Suspense fallback=move || {
                view! {
                    <div class="grid gap-3">
                        <Skeleton class="h-28 w-full"/>
                        <Skeleton class="h-28 w-full"/>
                    </div>
                }
            }>
                {move || {
                    Suspend::new(async move {
                        let sessions_list = sessions.await.unwrap_or_default();
                        match projects.await {
                            Err(e) => {
                                view! {
                                    <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                                        {e.to_string()}
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! {
                                    <p class="text-sm text-muted-foreground">
                                        "No projects yet."
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(list) => {
                                view! {
                                    <ul class="grid gap-3">
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
    let on_delete = move || {
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
        <li>
            <Card>
                <CardHeader class="flex-row items-center justify-between gap-3">
                    <CardTitle>{project.name.clone()}</CardTitle>
                    <div class="flex items-center gap-1.5">
                        {project
                            .node
                            .clone()
                            .map(|n| view! { <Badge variant=BadgeVariant::Info>{n}</Badge> })}
                        <Badge variant=BadgeVariant::Secondary>
                            {format!("{count} sessions")}
                        </Badge>
                    </div>
                </CardHeader>
                <CardContent class="flex flex-col gap-3">
                    <div class="flex items-center justify-between gap-3">
                        <p class="font-mono text-xs text-muted-foreground">
                            {project.id.clone()}
                        </p>
                        <Button
                            variant=ButtonVariant::Destructive
                            size=ButtonSize::Sm
                            disabled=move || deleting.get()
                            on_click=Box::new(on_delete)
                        >
                            {move || if deleting.get() { "Deleting…" } else { "Delete" }}
                        </Button>
                    </div>
                    {move || {
                        error
                            .get()
                            .map(|e| view! { <p class="text-sm text-destructive">{e}</p> })
                    }}
                </CardContent>
            </Card>
        </li>
    }
}
