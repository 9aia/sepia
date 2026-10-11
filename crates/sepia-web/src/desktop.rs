//! The desktop aim — the rail footer's "where New session points"
//! menu (the old ClientBar's desktop half; the browser-client
//! keypair identity it also carried isn't on the wire).
//!
//! [`Desktop`] is an `App`-provided `RwSignal<DesktopAim>` persisted
//! to localStorage (`sepia-desktop`) — the menu writes it, the
//! sessions panel's "New session" create reads it directly. The menu
//! itself is the
//! codebase's `<details>`-menu pattern (works without wasm, SSR and
//! hydrate emit identical markup): node submenus list each reachable
//! node's agent roster (a pick sets node+agent together), and a
//! Directory submenu offers the focused node's recent session dirs.
//! The wire exposes no model catalog, so `model` stays a free-text
//! form field — the aim still round-trips one for the settings UI.

use leptos::prelude::*;
use leptos_router::components::A;
use sepia_web_core::desktop::{
    DesktopAim, DotState, RECENT_DIR_LIMIT, dir_label, dot_state, effective_node, recent_cwds,
};

use crate::components::icons::Icon;
use crate::dto::{AgentDto, NodeStatusDto};

/// The shared aim — `App` provides it; `DesktopMenu` writes it and
/// sessions panel's create reads it. Starts unscoped on both targets; the
/// stored aim is adopted post-hydration like `PanelOpen`.
#[derive(Clone, Copy)]
pub struct Desktop(pub RwSignal<DesktopAim>);

/// Provide `Desktop` + (hydrate only) the seed-then-persist Effect —
/// same adoption dance as `provide_panel`.
pub fn provide_desktop() {
    let aim = RwSignal::new(DesktopAim::default());
    provide_context(Desktop(aim));
    #[cfg(feature = "hydrate")]
    {
        let (stored, set_stored, _clear) = leptos_use::storage::use_local_storage::<
            DesktopAim,
            codee::string::JsonSerdeCodec,
        >(sepia_web_core::desktop::STORAGE_KEY);
        let mut init = true;
        Effect::new(move |_| {
            let cur = aim.get();
            if std::mem::replace(&mut init, false) {
                aim.set(stored.get_untracked());
            } else {
                set_stored.set(cur);
            }
        });
    }
}

/// The rail's shared item geometry — same cells `app::RAIL_ITEM` uses.
const RAIL_ITEM: &str = "inline-flex size-9 items-center justify-center rounded-lg transition-colors \
                         focus-visible:ring-2 focus-visible:ring-sidebar-ring";

/// Menu rows — the old app's `rounded-2xl` menu-item recipe (same
/// class string `components::MenuItem` carries).
const ITEM: &str = "flex w-full items-center gap-2.5 rounded-2xl px-3 py-2 text-sm font-medium hover:bg-accent \
     cursor-pointer";
const ITEM_DISABLED: &str = "flex w-full items-center gap-2.5 rounded-2xl px-3 py-2 text-sm \
                             font-medium text-muted-foreground cursor-default";

/// Inline `onclick` for pick rows — closes the whole menu tree, not
/// just the submenu the row sits in (`closest('details')` stops at
/// the submenu). Runs pre-hydration like the backdrop's handler.
const CLOSE_ALL: &str = "var r=this.closest('[data-aim-menu]');\
                         r.querySelectorAll('details').forEach(function(d){d.removeAttribute('open')});\
                         r.removeAttribute('open')";

/// The backdrop's variant — `this` IS the backdrop; its parent is the
/// outer `<details>`.
const CLOSE_BACKDROP: &str = "var r=this.parentElement;\
                             r.querySelectorAll('details').forEach(function(d){d.removeAttribute('open')});\
                             r.removeAttribute('open')";

/// A node's display label — its status `label`, or the bare id.
fn row_label(row: &NodeStatusDto) -> String {
    if row.label.trim().is_empty() {
        row.id.clone()
    } else {
        row.label.clone()
    }
}

/// An agent's display label — `label`, or the bare id.
fn agent_label(a: &AgentDto) -> String {
    if a.label.trim().is_empty() {
        a.id.clone()
    } else {
        a.label.clone()
    }
}

/// One pick row inside the menu/submenus — closes the whole tree on
/// activation (`CLOSE_ALL`; `keep_open` doesn't exist here — picks
/// are terminal, like the old `DropdownMenuItem`s).
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
fn AimItem(
    /// Lucide name for a leading glyph (empty → text-only row).
    #[prop(into, optional)]
    icon: String,
    /// Trailing `✓` — the row is the aim's current pick.
    #[prop(optional)]
    checked: bool,
    /// Renders muted and inert (down node, empty submenu filler).
    #[prop(optional)]
    disabled: bool,
    /// `title` tooltip (dir rows carry the full path).
    #[prop(into, optional)]
    title: String,
    #[prop(optional)] on_click: Option<Box<dyn Fn() + Send + Sync + 'static>>,
    children: Children,
) -> impl IntoView {
    let cls = if disabled { ITEM_DISABLED } else { ITEM };
    view! {
        <button
            type="button"
            class=cls
            disabled=disabled
            title=(!title.is_empty()).then_some(title)
            onclick=(!disabled).then_some(CLOSE_ALL)
            on:click=move |ev| {
                if let Some(cb) = &on_click {
                    ev.prevent_default();
                    cb();
                }
            }
        >
            {(!icon.is_empty()).then(|| {
                view! { <Icon name=icon class="size-4 shrink-0 text-muted-foreground"/> }
            })}
            <span class="min-w-0 flex-1 truncate text-left">{children()}</span>
            {checked.then(|| {
                view! { <Icon name="check" class="size-3.5 shrink-0 text-primary"/> }
            })}
        </button>
    }
}

/// A `<details>` submenu — summary row (icon + label + current value
/// + chevron) with the panel flying out to the right. `disabled`
/// keeps it shut (a down node's roster isn't pickable).
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
fn AimSubMenu(
    /// Lucide name for the leading glyph.
    #[prop(into)]
    icon: String,
    #[prop(into)] label: String,
    /// The current pick, right-aligned in muted text (the old app's
    /// `SubValue` slot).
    #[prop(into, optional)]
    value: String,
    /// `data-name` hook for tests/e2e.
    #[prop(into, optional)]
    data_name: String,
    #[prop(optional)] disabled: bool,
    /// Trailing `✓` — this submenu's node is the focused one.
    #[prop(optional)]
    checked: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <details class="group/sub relative" data-name=data_name>
            <summary
                class=move || {
                    if disabled {
                        format!("{ITEM_DISABLED} list-none select-none [&::-webkit-details-marker]:hidden")
                    } else {
                        format!("{ITEM} list-none select-none [&::-webkit-details-marker]:hidden")
                    }
                }
                aria-disabled=disabled.then_some("true")
            >
                <Icon name=icon class="size-4 shrink-0 text-muted-foreground"/>
                <span class="flex min-w-0 flex-1 items-baseline justify-between gap-2">
                    <span class="shrink-0">{label}</span>
                    <span class="truncate text-xs font-normal text-muted-foreground">{value}</span>
                </span>
                {checked.then(|| {
                    view! { <Icon name="check" class="size-3.5 shrink-0 text-primary"/> }
                })}
                <Icon
                    name="chevron-right"
                    class="size-3.5 shrink-0 text-muted-foreground transition-transform group-open/sub:rotate-90"
                />
            </summary>
            <div class="absolute -top-1.5 left-full z-50 w-52 rounded-3xl bg-popover p-1.5 text-sm text-popover-foreground shadow-lg ring-1 ring-border">
                {children()}
            </div>
        </details>
    }
}

/// The rail footer's desktop menu — the old ClientBar, compressed to
/// the icon rail: the trigger is a `size-9` cell with the reachability
/// dot on its corner (the tooltip carries the compact
/// `{node} · {agent}` summary the old trigger row showed).
///
/// Menu, top to bottom: the summary line, a submenu per registered
/// node listing that node's agent roster (a pick sets node+agent —
/// the old `pick(node, agent)` drops scoped fields that don't port),
/// a Directory submenu with the focused node's recent session cwds
/// (the wire has no dir/model catalogs, so model stays a form field),
/// "Reset desktop" while anything is picked, then Settings +
/// Keyboard shortcuts like the old menu's tail.
///
/// Data comes from the shared `leptos-fetch` cache, mirrored into
/// plain signals by hydrate-only Effects — Resource reads outside a
/// `Suspense` warn (and risk SSR/hydrate divergence), so the view
/// never touches them directly. SSR and first hydration both render
/// the empty mirrors ("Checking nodes"), rosters land reactively.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn DesktopMenu(
    /// The `?` cheat-sheet's open flag — the menu's Keyboard
    /// shortcuts item flips it (the old menu's `hasKeyboard` row;
    /// the rail is `lg+`-only anyway).
    help: RwSignal<bool>,
) -> impl IntoView {
    let aim = expect_context::<Desktop>().0;
    // The mirrors — empty on SSR and first hydration; the Effects
    // below fill them from the shared query cache post-mount.
    let statuses = RwSignal::new(Vec::<NodeStatusDto>::new());
    let info = RwSignal::new(crate::dto::NodeInfoDto::default());
    let agents = RwSignal::new(Vec::<AgentDto>::new());
    let sessions = RwSignal::new(Vec::<crate::dto::SessionSummaryDto>::new());
    // Whether the status query has resolved at all (Ok or Err) —
    // gates the dot's Checking state; the list pages do the same
    // with their resource handles.
    let probe_loaded = RwSignal::new(false);

    // The query handles exist on both targets — resource creation
    // must stay symmetric so hydrate-side indexing matches the
    // serialized SSR pool (hydrate-only handles shift every later
    // resource's id and break Suspense adoption).
    let client = crate::api::query_client();
    let status_q = client.resource(crate::api::node_status_scope, || ());
    let info_q = client.resource(crate::api::node_info_scope, || ());
    let agents_q = client.resource(crate::api::agents_scope, || ());
    let sessions_q = client.resource(crate::api::sessions_scope, || ());

    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (status_q, info_q, agents_q, sessions_q);
    }
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            let q = status_q.get();
            let resolved = q.is_some();
            statuses.set(q.and_then(Result::ok).unwrap_or_default());
            probe_loaded.set(resolved);
        });
        Effect::new(move |_| {
            if let Some(n) = info_q.get().and_then(Result::ok) {
                info.set(n);
            }
        });
        Effect::new(move |_| {
            agents.set(agents_q.get().and_then(Result::ok).unwrap_or_default());
        });
        Effect::new(move |_| {
            sessions.set(sessions_q.get().and_then(Result::ok).unwrap_or_default());
        });
        // Freshness — the node feed + a slow poll for reachability,
        // the same triggers `SessionListPanel` uses for these scopes.
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || {
            status_q.refetch();
            sessions_q.refetch();
        }));
        crate::app::every_ms(30_000, move || status_q.refetch());
    }

    // Query reads — each returns the mirrored rows ([]/None until
    // the fill Effects land).
    let status_rows = move || statuses.get();
    let primary = move || {
        let n = info.get();
        (!n.id.is_empty()).then_some(n.id)
    };
    let agent_list = move || agents.get();
    let session_list = move || sessions.get();

    // The focused node's id — the aim's pick, else the hub's primary,
    // else the first status row (`node=None` on a create means "the
    // primary/first" node-side, so the first row is the honest guess
    // when even the descriptor hasn't landed).
    let focused_id = move || {
        let a = aim.get();
        if let Some(n) = &a.node {
            return Some(n.clone());
        }
        let rows = status_rows();
        let p = primary();
        // The descriptor's self-id (`node_…`) isn't the registry id
        // (`main`) — prefer a registered row so the same node doesn't
        // surface twice ("a16x — unavailable" next to a healthy
        // `main`). Only when no rows landed does the raw self-id win.
        if let Some(row) = rows.iter().find(|r| Some(&r.id) == p.as_ref()) {
            return Some(row.id.clone());
        }
        rows.first().map(|r| r.id.clone()).or(p)
    };

    // Label for an id — the status row's `label`, the node
    // descriptor's `name` for a primary missing from the registry,
    // or the raw id.
    let node_label = move |id: &str| -> String {
        let rows = status_rows();
        if let Some(r) = rows.iter().find(|r| r.id == id) {
            return row_label(r);
        }
        if primary().as_deref() == Some(id) {
            let n = info.get();
            return if n.name.trim().is_empty() {
                n.id.clone()
            } else {
                n.name.clone()
            };
        }
        id.to_string()
    };

    // The dot + summary — the old `dotState`/`dotLabel` pair.
    let probe = move || {
        let loaded = probe_loaded.get();
        let rows = status_rows();
        let any_up = rows.iter().any(|r| r.status == "up");
        let fid = focused_id();
        let focused = fid
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id).map(|r| r.status.as_str()));
        (dot_state(loaded, any_up, focused), any_up, fid)
    };
    let dot_label = move || {
        let (state, any_up, fid) = probe();
        let label = fid
            .as_deref()
            .map_or_else(|| "nodes".to_string(), node_label);
        match state {
            DotState::Offline if !any_up => "No nodes connected".to_string(),
            DotState::Offline => format!("{label} unreachable"),
            DotState::Checking => format!("Checking {label}"),
            DotState::Online => format!("{label} online"),
        }
    };

    view! {
        <details class="group relative" data-name="DesktopMenu" data-aim-menu>
            <summary
                class=format!("{RAIL_ITEM} cursor-pointer select-none text-sidebar-foreground/60 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground [&::-webkit-details-marker]:hidden group-open:bg-sidebar-accent group-open:text-sidebar-accent-foreground")
                aria-label=move || format!("Desktop — {}", dot_label())
                title=dot_label
            >
                <span class="relative">
                    <Icon name="monitor" class="size-5"/>
                    <span
                        role="status"
                        data-name="AimDot"
                        aria-label=dot_label
                        class=move || {
                            let (state, _, _) = probe();
                            let base = "absolute -right-0.5 -bottom-0.5 size-2 rounded-full border-2 border-sidebar";
                            match state {
                                DotState::Online => format!("{base} bg-emerald-500"),
                                DotState::Offline => format!("{base} animate-pulse bg-destructive"),
                                DotState::Checking => {
                                    format!("{base} bg-muted-foreground/50")
                                }
                            }
                        }
                    ></span>
                </span>
            </summary>
            // open: sibling backdrop swallows the outside click and
            // folds the whole tree (submenus included).
            <span
                class="fixed inset-0 z-40 hidden cursor-default group-open:block"
                onclick=CLOSE_BACKDROP
            ></span>
            <div class="absolute bottom-full left-0 z-50 mb-1 w-60 rounded-3xl bg-popover p-1.5 text-sm text-popover-foreground shadow-lg ring-1 ring-border">
                <div
                    class="truncate px-3 pb-1 pt-1.5 text-xs font-medium text-muted-foreground"
                    data-name="AimSummary"
                >
                    {move || dot_label()}
                </div>
                {move || {
                    let rows = status_rows();
                    let loaded = probe_loaded.get();
                    let primary_now = primary();
                    let fid = focused_id();
                    if rows.is_empty() {
                        // Unloaded probes and dead hubs render the same
                        // inert row — the empty-list gate reads the same
                        // way.
                        let text = if loaded {
                            "No nodes connected"
                        } else {
                            "Checking nodes…"
                        };
                        view! {
                            <button type="button" class=ITEM_DISABLED disabled=true>
                                <span class="min-w-0 flex-1 truncate text-left">{text}</span>
                            </button>
                        }
                            .into_any()
                    } else {
                        let agents_now = agent_list();
                        rows.iter()
                            .map(|row| {
                                let id = row.id.clone();
                                let label = row_label(row);
                                let down = row.status == "down";
                                let is_focused = fid.as_deref() == Some(id.as_str());
                                // The node's roster — bare `node`
                                // annotations belong to the primary.
                                let bucket = Some(id.clone());
                                let roster: Vec<AgentDto> = agents_now
                                    .iter()
                                    .filter(|a| {
                                        effective_node(&a.node, &primary_now) == bucket
                                    })
                                    .cloned()
                                    .collect();
                                let aim_now = aim.get();
                                let node_for_pick = id.clone();
                                view! {
                                    <AimSubMenu
                                        icon="server"
                                        label=label
                                        value=row.status.clone()
                                        data_name="AimNodeSubmenu"
                                        disabled=down
                                        checked=is_focused
                                    >
                                        <AimItem
                                            checked=is_focused && aim_now.agent.is_none()
                                            on_click=Box::new({
                                                let node = node_for_pick.clone();
                                                move || {
                                                    aim.update(|a| {
                                                        a.pick(Some(node.clone()), None);
                                                    });
                                                }
                                            })
                                        >
                                            "Node default"
                                        </AimItem>
                                        {if roster.is_empty() {
                                            view! {
                                                <button
                                                    type="button"
                                                    class=ITEM_DISABLED
                                                    disabled=true
                                                >
                                                    <span class="min-w-0 flex-1 truncate text-left">
                                                        "No agents"
                                                    </span>
                                                </button>
                                            }
                                                .into_any()
                                        } else {
                                            roster
                                                .into_iter()
                                                .map(|agent| {
                                                    let agent_id = agent.id.clone();
                                                    let picked =
                                                        is_focused
                                                            && aim_now.agent.as_deref()
                                                                == Some(agent_id.as_str());
                                                    let node = node_for_pick.clone();
                                                    view! {
                                                        <AimItem
                                                            checked=picked
                                                            on_click=Box::new(move || {
                                                                aim.update(|a| {
                                                                    a.pick(
                                                                        Some(node.clone()),
                                                                        Some(agent_id.clone()),
                                                                    );
                                                                });
                                                            })
                                                        >
                                                            {agent_label(&agent)}
                                                        </AimItem>
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .into_any()
                                        }}
                                    </AimSubMenu>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }
                }}
                // The aim names a node the registry doesn't (disabled
                // or gone) — show it so the ✓ isn't orphaned, like the
                // old `nodeMissing` row.
                {move || {
                    let fid = focused_id();
                    let rows = status_rows();
                    let missing = fid
                        .as_deref()
                        .is_some_and(|id| rows.iter().all(|r| r.id != id));
                    missing.then(|| {
                        let label =
                            fid.as_deref().map_or_else(String::new, node_label);
                        view! {
                            <button type="button" class=ITEM_DISABLED disabled=true>
                                <span class="min-w-0 flex-1 truncate text-left">
                                    {format!("{label} — unavailable")}
                                </span>
                            </button>
                        }
                    })
                }}
                {move || {
                    let dir_submenu = {
                        let aim_now = aim.get();
                        let primary_now = primary();
                        let fid = focused_id();
                        let recents = recent_cwds(
                            session_list()
                                .iter()
                                .map(|s| (s.cwd.as_str(), &s.node)),
                            &fid,
                            &primary_now,
                            RECENT_DIR_LIMIT,
                        );
                        let value = aim_now
                            .cwd
                            .as_deref()
                            .map_or_else(|| "Most recent".to_string(), dir_label);
                        view! {
                            <AimSubMenu
                                icon="folder"
                                label="Directory"
                                value=value
                                data_name="AimDirSubmenu"
                            >
                                <AimItem
                                    checked=aim_now.cwd.is_none()
                                    on_click=Box::new(move || {
                                        aim.update(|a| a.cwd = None);
                                    })
                                >
                                    "Most recent"
                                </AimItem>
                                {if recents.is_empty() {
                                    view! {
                                        <button
                                            type="button"
                                            class=ITEM_DISABLED
                                            disabled=true
                                        >
                                            <span class="min-w-0 flex-1 truncate text-left">
                                                "No known directories"
                                            </span>
                                        </button>
                                    }
                                        .into_any()
                                } else {
                                    recents
                                        .into_iter()
                                        .map(|dir| {
                                            let pick = dir.clone();
                                            let title = dir.clone();
                                            let picked =
                                                aim_now.cwd.as_deref() == Some(dir.as_str());
                                            view! {
                                                <AimItem
                                                    checked=picked
                                                    title=title
                                                    on_click=Box::new(move || {
                                                        aim.update(|a| {
                                                            a.cwd = Some(pick.clone());
                                                        });
                                                    })
                                                >
                                                    {dir_label(&dir)}
                                                </AimItem>
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .into_any()
                                }}
                            </AimSubMenu>
                        }
                    };
                    dir_submenu.into_any()
                }}
                {move || {
                    aim.get().is_set().then(|| {
                        view! {
                            <AimItem
                                icon="rotate-ccw"
                                on_click=Box::new(move || {
                                    aim.update(DesktopAim::clear);
                                })
                            >
                                "Reset desktop"
                            </AimItem>
                        }
                    })
                }}
                <hr class="my-1 h-px border-none bg-border"/>
                <A
                    href="/settings"
                    attr:data-name="AimSettings"
                    attr:class=ITEM
                    attr:onclick=CLOSE_ALL
                >
                    <Icon name="settings" class="size-4 shrink-0 text-muted-foreground"/>
                    <span class="min-w-0 flex-1 truncate text-left">"Settings"</span>
                </A>
                <button
                    type="button"
                    class=ITEM
                    data-name="AimShortcuts"
                    onclick=CLOSE_ALL
                    on:click=move |_| help.set(true)
                >
                    <Icon name="keyboard" class="size-4 shrink-0 text-muted-foreground"/>
                    <span class="min-w-0 flex-1 truncate text-left">"Keyboard shortcuts"</span>
                    <kbd class="rounded border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
                        "?"
                    </kbd>
                </button>
            </div>
        </details>
    }
        .into_any()
}
