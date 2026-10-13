//! `TransferSheet` — the node-picker dialog behind every "Transfer…"
//! affordance (session row menu, cwd-group menu, details sheet,
//! project details). The hub orchestrates export → import
//! (`docs/transfer.md`); this sheet only picks the target node and an
//! optional cwd remap. SSR emits the closed shell like `Sheet` — the
//! open flag is the only behavior.

use leptos::prelude::*;

use crate::components::icons::Icon;
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Input, Sheet, SheetBody, SheetHeader,
    SheetTitle,
};
use crate::dto::NodeStatusDto;

/// The shared node-picker sheet. `targets` is already source-free
/// (callers filter the owning node out); `cwd_default` prefills the
/// remap field — submit sends its content verbatim, so leaving it
/// unchanged lands the copy at the same path the source used. `busy`
/// is caller-owned: the transfer call is async and the button reflects
/// it.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn TransferSheet(
    open: RwSignal<bool>,
    /// Sheet heading — "Transfer session" / "Transfer project".
    #[prop(into)]
    heading: String,
    /// One-line explainer under the heading.
    #[prop(into)]
    description: String,
    /// Candidate target nodes.
    targets: Signal<Vec<NodeStatusDto>>,
    /// Prefill for the cwd field — the path the source session(s) run
    /// under. Edit to remap onto the target machine's layout. A Signal
    /// because the sheet is shared: the clicked row's cwd lands here.
    #[prop(into, optional)]
    cwd_default: Signal<String>,
    /// `id` stem for the cwd input (pages mounting two sheets need
    /// distinct ids for the label's `for`).
    #[prop(into, optional)]
    field_id: String,
    busy: RwSignal<bool>,
    /// `(target node id, cwd remap)` — the caller owns the async call,
    /// toasts, refetches, and closing the sheet.
    on_submit: Box<dyn Fn(String, Option<String>) + Send + Sync + 'static>,
) -> impl IntoView {
    let picked: RwSignal<Option<String>> = RwSignal::new(None);
    let cwd = RwSignal::new(cwd_default.get_untracked());
    let field_id = if field_id.is_empty() {
        "transfer-cwd".to_string()
    } else {
        field_id
    };
    let toast = use_toast();
    // Re-opening starts clean — the field re-adopts the *current*
    // source cwd (the sheet is shared across rows). The Effect's
    // first run fires on hydrate mount too; everything's already
    // empty/prefilled then.
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        if open.get() {
            picked.set(None);
            cwd.set(cwd_default.get());
        }
    });

    let on_submit = std::sync::Arc::new(on_submit);
    view! {
        <Sheet open=open side="right" class="w-96" label=heading.clone()>
            <SheetHeader>
                <SheetTitle>{heading}</SheetTitle>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Icon
                    class="size-7"
                    on_click=Box::new(move || open.set(false))
                >
                    <Icon name="x" class="size-4"/>
                </Button>
            </SheetHeader>
            <SheetBody class="space-y-4">
                <p class="text-xs text-muted-foreground">{description}</p>
                <div class="space-y-1.5">
                    <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                        "Target node"
                    </label>
                    <ul class="space-y-1">
                        {move || {
                            let list = targets.get();
                            if list.is_empty() {
                                return view! {
                                    <li class="text-sm text-muted-foreground">
                                        "No other nodes registered."
                                    </li>
                                }
                                    .into_any();
                            }
                            list.into_iter()
                                .map(|n| {
                                    let id = n.id.clone();
                                    let id_class = id.clone();
                                    let down = n.status == "down";
                                    let label = if n.label.is_empty() {
                                        n.id.clone()
                                    } else {
                                        n.label.clone()
                                    };
                                    view! {
                                        <li>
                                            <button
                                                type="button"
                                                disabled=down
                                                class=move || {
                                                    let base = "flex w-full items-center gap-2 rounded-2xl px-3 py-2 text-left text-sm transition-colors";
                                                    if down {
                                                        format!(
                                                            "{base} cursor-not-allowed text-muted-foreground/50"
                                                        )
                                                    } else if picked.get().as_deref()
                                                        == Some(id_class.as_str())
                                                    {
                                                        format!("{base} bg-accent ring-1 ring-primary/40")
                                                    } else {
                                                        format!("{base} hover:bg-accent/60")
                                                    }
                                                }
                                                on:click=move |_| {
                                                    if !down {
                                                        picked.set(Some(id.clone()));
                                                    }
                                                }
                                            >
                                                <Icon name="server" class="size-4 text-muted-foreground"/>
                                                <span class="min-w-0 flex-1">
                                                    <span class="block truncate font-medium">
                                                        {label}
                                                    </span>
                                                    <span class="block truncate text-xs text-muted-foreground">
                                                        {n.url.clone()}
                                                    </span>
                                                </span>
                                                {if down {
                                                    view! {
                                                        <Badge variant=BadgeVariant::Muted>
                                                            "down"
                                                        </Badge>
                                                    }
                                                        .into_any()
                                                } else if n.status != "up" {
                                                    view! {
                                                        <Badge variant=BadgeVariant::Outline>
                                                            {n.status.clone()}
                                                        </Badge>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! { <span></span> }.into_any()
                                                }}
                                            </button>
                                        </li>
                                    }
                                })
                                .collect::<Vec<_>>()
                                .into_any()
                        }}
                    </ul>
                </div>
                <div class="space-y-1.5">
                    <label
                        class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
                        for=field_id.clone()
                    >
                        "Directory on target"
                    </label>
                    <Input
                        attr:id=field_id
                        attr:r#type="text"
                        attr:placeholder="Working directory on the target"
                        prop:value=move || cwd.get()
                        on:input=move |ev| cwd.set(event_target_value(&ev))
                    />
                    <p class="text-xs text-muted-foreground">
                        "The session copy resumes under this path — it must exist on the target machine (clone/sync the checkout yourself)."
                    </p>
                </div>
                <div class="flex justify-end gap-2 pt-1">
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on_click=Box::new(move || open.set(false))
                    >
                        "Cancel"
                    </Button>
                    <Button
                        size=ButtonSize::Sm
                        disabled=move || busy.get() || picked.get().is_none()
                        on_click=Box::new(move || {
                            let Some(target) = picked.get_untracked() else {
                                return;
                            };
                            if busy.get_untracked() {
                                return;
                            }
                            let remap =
                                sepia_web_core::path::non_empty(&cwd.get_untracked());
                            if remap.is_none() {
                                toast.info("No target directory — the session's stored cwd is kept.");
                            }
                            let on_submit = std::sync::Arc::clone(&on_submit);
                            on_submit(target, remap);
                        })
                    >
                        {move || if busy.get() { "Transferring…" } else { "Transfer" }}
                    </Button>
                </div>
            </SheetBody>
        </Sheet>
    }
}
