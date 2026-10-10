//! `/projects` — named groups over session meta. Lists are merged
//! across nodes on a hub (each row carries a `node` badge); creates go
//! to the primary node, deletes to the owning one. Each project's
//! details sheet edits membership — `projectIds` on the session meta
//! overlay (`set_session_projects`).

use leptos::prelude::*;
use leptos_meta::Title;
use sepia_web_core::project::{is_member, with_membership};

use crate::api::{create_project, delete_project, set_session_projects};
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardHeader,
    CardTitle, ConfirmDialog, ErrorBanner, Input, PageDescription, PageHead, PageTitle, Sheet,
    SheetBody, SheetHeader, SheetTitle, Skeleton,
};
use crate::dto::{ProjectDto, SessionSummaryDto};

#[component]
pub fn ProjectsPage() -> impl IntoView {
    let client = crate::api::query_client();
    let projects = client.resource(crate::api::projects_scope, || ());
    let sessions = client.resource(crate::api::sessions_scope, || ());
    let toast = use_toast();

    let draft = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let form_error: RwSignal<Option<String>> = RwSignal::new(None);

    // Project details sheet + the shared delete confirm — one
    // ConfirmDialog instance at page level so `aria-labelledby` ids
    // stay unique.
    let detail: RwSignal<Option<ProjectDto>> = RwSignal::new(None);
    let detail_open = RwSignal::new(false);
    let confirm_open = RwSignal::new(false);
    let delete_target: RwSignal<Option<ProjectDto>> = RwSignal::new(None);

    let create = move || {
        let name = draft.get();
        if name.trim().is_empty() || busy.get() {
            return;
        }
        busy.set(true);
        form_error.set(None);
        leptos::task::spawn_local(async move {
            match create_project(name.clone(), None).await {
                Ok(p) => {
                    toast.success(format!("Created project \"{}\".", p.name));
                    draft.set(String::new());
                    projects.refetch();
                }
                Err(e) => {
                    let msg = e.to_string();
                    toast.error(msg.clone());
                    form_error.set(Some(msg));
                }
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
                form_error
                    .get()
                    .map(|e| view! { <ErrorBanner message=e/> })
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
                                    <ErrorBanner
                                        message=e.to_string()
                                        on_retry=Box::new(move || projects.refetch())
                                    />
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
                                                        selected=detail
                                                        sheet_open=detail_open
                                                        confirm_open=confirm_open
                                                        delete_target=delete_target
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
        <Sheet open=detail_open side="right" class="w-96" label="Project details">
            <SheetHeader>
                <SheetTitle>
                    {move || {
                        detail
                            .get()
                            .map_or_else(|| "Project".to_string(), |p| p.name)
                    }}
                </SheetTitle>
            </SheetHeader>
            <SheetBody>
                {move || {
                    match detail.get() {
                        None => {
                            view! { <p class="text-sm text-muted-foreground">"—"</p> }
                                .into_any()
                        }
                        Some(p) => {
                            view! {
                                <ProjectDetails
                                    project=p
                                    sessions=sessions
                                    confirm_open=confirm_open
                                    delete_target=delete_target
                                />
                            }
                                .into_any()
                        }
                    }
                }}
            </SheetBody>
        </Sheet>
        <ConfirmDialog
            open=confirm_open
            id="project-delete"
            title="Delete this project?"
            body="The project is removed from the registry; its sessions keep their history but lose the tag."
            confirm_label="Delete"
            destructive=true
            on_confirm=move || {
                if let Some(p) = delete_target.get() {
                    delete_target.set(None);
                    detail_open.set(false);
                    detail.set(None);
                    leptos::task::spawn_local(async move {
                        if toast
                            .outcome(
                                delete_project(p.id.clone(), p.node.clone()).await,
                                format!("Deleted project \"{}\".", p.name),
                            )
                            .is_some()
                        {
                            client.invalidate_query(crate::api::projects_scope, ());
                            client.invalidate_query(crate::api::sessions_scope, ());
                        }
                    });
                }
            }
        />
    }
}

/// Sessions whose `projectIds` mention this project — node ids
/// disambiguate when the merged list carries them (`project::is_member`).
fn session_count(sessions: &[SessionSummaryDto], project: &ProjectDto) -> usize {
    sessions
        .iter()
        .filter(|s| {
            is_member(
                &s.project_ids,
                s.node.as_deref(),
                &project.id,
                project.node.as_deref(),
            )
        })
        .count()
}

/// The details sheet body — member-session editor + delete action.
/// `sessions.get()` is `None` until the shared query resolves; the
/// sheet's own open flag keeps this markup client-only.
#[component]
fn ProjectDetails(
    project: ProjectDto,
    sessions: Resource<Result<Vec<SessionSummaryDto>, ServerFnError>>,
    confirm_open: RwSignal<bool>,
    delete_target: RwSignal<Option<ProjectDto>>,
) -> impl IntoView {
    let toast = use_toast();
    let client = crate::api::query_client();
    // Disjoint clones for each `move` closure below — `project` can't
    // be captured by value in several places at once.
    let project_id = project.id.clone();
    let project_node = project.node.clone();
    let project_for_delete = project.clone();
    view! {
        <div data-name="ProjectDetails" class="flex flex-col gap-4">
            <p class="font-mono text-xs text-muted-foreground">
                {project.id.clone()}
            </p>
            <div>
                <h4 class="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                    "Sessions"
                </h4>
                {move || {
                    match sessions.get() {
                        None => {
                            view! { <Skeleton class="h-20 w-full"/> }.into_any()
                        }
                        Some(Err(e)) => {
                            view! { <ErrorBanner message=e.to_string()/> }.into_any()
                        }
                        Some(Ok(list)) => {
                            let members: Vec<SessionSummaryDto> = list
                                .iter()
                                .filter(|s| {
                                    project_node.is_none() || s.node == project_node
                                })
                                .cloned()
                                .collect();
                            if members.is_empty() {
                                view! {
                                    <p class="text-sm text-muted-foreground">
                                        "No sessions on this node."
                                    </p>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <ul class="divide-y divide-border">
                                        {members
                                            .into_iter()
                                            .map(|s| {
                                                let member = is_member(
                                                    &s.project_ids,
                                                    s.node.as_deref(),
                                                    &project_id,
                                                    project_node.as_deref(),
                                                );
                                                let title = if s.title.is_empty() {
                                                    s.id.clone()
                                                } else {
                                                    s.title.clone()
                                                };
                                                let session_id = s.id.clone();
                                                let agent = s.agent.clone();
                                                let project_id = project_id.clone();
                                                let current = s.project_ids.clone();
                                                view! {
                                                    <li class="py-2 first:pt-0 last:pb-0">
                                                        <label class="flex items-center gap-2 text-sm">
                                                            <input
                                                                type="checkbox"
                                                                class="size-4 accent-primary"
                                                                prop:checked=member
                                                                on:change=move |ev| {
                                                                    let on = event_target_checked(&ev);
                                                                    let ids = with_membership(
                                                                        &current,
                                                                        &project_id,
                                                                        on,
                                                                    );
                                                                    if ids == current {
                                                                        return;
                                                                    }
                                                                    let session_id = session_id.clone();
                                                                    let agent = if agent.is_empty() {
                                                                        None
                                                                    } else {
                                                                        Some(agent.clone())
                                                                    };
                                                                    leptos::task::spawn_local(async move {
                                                                        let res = set_session_projects(
                                                                            session_id,
                                                                            agent,
                                                                            ids,
                                                                        )
                                                                        .await;
                                                                        if toast
                                                                            .outcome(
                                                                                res,
                                                                                "Project membership updated.",
                                                                            )
                                                                            .is_some()
                                                                        {
                                                                            client.invalidate_query(
                                                                                crate::api::sessions_scope,
                                                                                (),
                                                                            );
                                                                        }
                                                                    });
                                                                }
                                                            />
                                                            <span class="min-w-0 flex-1 truncate">{title}</span>
                                                            <code class="shrink-0 font-mono text-[10px] text-muted-foreground">
                                                                {s.agent.clone()}
                                                            </code>
                                                        </label>
                                                    </li>
                                                }
                                            })
                                            .collect::<Vec<_>>()}
                                    </ul>
                                }
                                    .into_any()
                            }
                        }
                    }
                }}
            </div>
            <div class="mt-auto border-t pt-4">
                <Button
                    variant=ButtonVariant::Destructive
                    size=ButtonSize::Sm
                    on_click=Box::new(move || {
                        delete_target.set(Some(project_for_delete.clone()));
                        confirm_open.set(true);
                    })
                >
                    "Delete project…"
                </Button>
            </div>
        </div>
    }
}

#[component]
fn ProjectRow(
    project: ProjectDto,
    count: usize,
    selected: RwSignal<Option<ProjectDto>>,
    sheet_open: RwSignal<bool>,
    confirm_open: RwSignal<bool>,
    delete_target: RwSignal<Option<ProjectDto>>,
) -> impl IntoView {
    // One clone per `move` closure — the details open and the delete
    // confirm each own one.
    let for_details = project.clone();
    let for_delete = project.clone();
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
                        <div class="flex items-center gap-2">
                            <Button
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                                on_click=Box::new(move || {
                                    selected.set(Some(for_details.clone()));
                                    sheet_open.set(true);
                                })
                            >
                                "Details"
                            </Button>
                            <Button
                                variant=ButtonVariant::Destructive
                                size=ButtonSize::Sm
                                on_click=Box::new(move || {
                                    delete_target.set(Some(for_delete.clone()));
                                    confirm_open.set(true);
                                })
                            >
                                "Delete"
                            </Button>
                        </div>
                    </div>
                </CardContent>
            </Card>
        </li>
    }
}
