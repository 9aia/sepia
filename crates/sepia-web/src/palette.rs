//! `⌘K`/`/` command palette — a centered `role="dialog"` overlay with
//! an input and a keyboard-navigable `role="listbox"` of session
//! jumps, page nav, and shell actions. Ranking is pure
//! (`sepia_web_core::palette::rank`); this file is event/view wiring.
//!
//! Always mounted like `ShortcutsHelp` — visibility is class-driven so
//! SSR and first hydration emit identical markup (closed).

use leptos::prelude::*;
use leptos_router::NavigateOptions;
#[cfg(feature = "hydrate")]
use leptos_router::hooks::use_navigate;
use leptos_router::hooks::use_query_map;
use sepia_web_core::filter::session_href;
use sepia_web_core::keymap::{self, NavDir};
use sepia_web_core::palette::{self, PaletteItem, PaletteKind};

#[cfg(feature = "hydrate")]
use crate::api::{archive_session, pin_session};
use crate::components::Input;
use crate::components::icons::Icon;
#[cfg(feature = "hydrate")]
use crate::components::toast::use_toast;
use crate::theme::Theme;

/// Static page-nav items — authored ahead of actions + sessions.
const PAGES: &[(&str, &str)] = &[
    ("/", "Sessions"),
    ("/agents", "Agents"),
    ("/projects", "Projects"),
    ("/nodes", "Nodes"),
    ("/settings", "Settings"),
];

/// Icon per row kind — session rows get the session glyph, pages use
/// their nav icon where one exists, actions the `+`.
fn row_icon(item: &PaletteItem) -> &'static str {
    match item.kind {
        PaletteKind::Session => "sessions",
        PaletteKind::Action => "plus",
        PaletteKind::Page => match item.target.as_str() {
            "/agents" => "agents",
            "/projects" => "projects",
            "/nodes" => "nodes",
            "/settings" => "settings",
            _ => "sessions",
        },
    }
}

#[component]
pub fn CommandPalette(open: RwSignal<bool>, help: RwSignal<bool>) -> impl IntoView {
    // Sessions land in a plain signal — populated by a lazy fetch on
    // open. No `client.resource`: a resource created during SSR spawns
    // a fetch outside any Suspense boundary (the parallel SSR tests
    // deadlock on it), and the palette can't open server-side anyway.
    let session_rows = RwSignal::new(Vec::<crate::dto::SessionSummaryDto>::new());
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let query_map = use_query_map();
    #[cfg(feature = "hydrate")]
    let toast = use_toast();
    let theme = expect_context::<RwSignal<Theme>>();
    // `use_navigate` isn't Send — same StoredValue dance as the
    // session list (SSR never navigates).
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    #[cfg(not(feature = "hydrate"))]
    let navigate = StoredValue::new(|_: &str, _: NavigateOptions| {});

    // The `?session=<id>&agent=<agent>` selection — pin/archive
    // actions target it and read the current flags off the row.
    let selected = move || {
        let q = query_map.read();
        let id = q.get("session").unwrap_or_default();
        (!id.is_empty()).then_some((id, q.get("agent").unwrap_or_default()))
    };

    // Everything the listbox can show, in authored order — pages,
    // actions, then sessions. `sessions.get()` returns `None` while
    // the query is pending (always on SSR), which is the desired
    // markup: the palette opens client-side.
    let items = move || -> Vec<PaletteItem> {
        let mut v: Vec<PaletteItem> = PAGES
            .iter()
            .map(|(p, t)| PaletteItem::page(*p, *t))
            .collect();
        v.push(PaletteItem::action(
            "new",
            "New session",
            "jump to the create form",
        ));
        v.push(PaletteItem::action(
            "theme",
            "Toggle theme",
            "cycle system → dark → light",
        ));
        v.push(PaletteItem::action("help", "Keyboard shortcuts", "?"));
        let list = session_rows.get();
        if let Some((id, _agent)) = selected() {
            if let Some(row) = list.iter().find(|s| s.id == id) {
                let title = if row.title.is_empty() {
                    row.id.clone()
                } else {
                    row.title.clone()
                };
                v.push(PaletteItem::action(
                    "pin",
                    if row.pinned {
                        format!("Unpin “{title}”")
                    } else {
                        format!("Pin “{title}”")
                    },
                    "selected session",
                ));
                v.push(PaletteItem::action(
                    "archive",
                    if row.archived {
                        format!("Unarchive “{title}”")
                    } else {
                        format!("Archive “{title}”")
                    },
                    "selected session",
                ));
            }
        }
        for s in list.iter().filter(|s| !s.archived) {
            let title = if s.title.is_empty() {
                s.id.clone()
            } else {
                s.title.clone()
            };
            let hint = [s.cwd.as_str(), s.agent.as_str()]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            v.push(PaletteItem::session(
                session_href(&s.id, &s.agent),
                title,
                hint,
            ));
        }
        v
    };

    // The currently ranked rows — shared by the listbox render and
    // the keyboard handler (Enter activates `active` within it).
    // Owned clones: the small list makes per-keystroke copies cheap.
    let ranked = move || -> Vec<PaletteItem> {
        palette::rank(&query.get(), &items())
            .into_iter()
            .cloned()
            .collect()
    };

    let activate = move |item: &PaletteItem| {
        open.set(false);
        let Some(action) = item.target.strip_prefix("action:") else {
            navigate.with_value(|n| n(&item.target, NavigateOptions::default()));
            return;
        };
        match action {
            "new" => navigate.with_value(|n| n("/", NavigateOptions::default())),
            "theme" => theme.update(|t| *t = t.next()),
            "help" => help.set(true),
            // Pin/archive act on the `?session=` selection — the item
            // only exists while one is selected.
            "pin" | "archive" => {
                #[cfg(feature = "hydrate")]
                {
                    let pin = action == "pin";
                    if let Some((id, agent)) = selected() {
                        let agent = (!agent.is_empty()).then_some(agent);
                        let cur = session_rows
                            .get_untracked()
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| (s.pinned, s.archived))
                            .unwrap_or_default();
                        let flag = if pin { !cur.0 } else { !cur.1 };
                        leptos::task::spawn_local(async move {
                            let res = if pin {
                                pin_session(id, agent, flag).await
                            } else {
                                archive_session(id, agent, flag).await
                            };
                            match res {
                                Ok(()) => {
                                    // Fresh flags for the next open.
                                    if let Ok(list) = crate::api::list_sessions().await {
                                        session_rows.set(list);
                                    }
                                }
                                Err(e) => toast.error(e.to_string()),
                            }
                        });
                    }
                }
            }
            _ => {}
        }
    };

    // On open: fresh query, focus the input (always mounted — no mount
    // race), and refresh the session list for jumps + pin/archive.
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        if open.get() {
            query.set(String::new());
            active.set(0);
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
            }
            leptos::task::spawn_local(async move {
                if let Ok(list) = crate::api::list_sessions().await {
                    session_rows.set(list);
                }
            });
        }
    });

    view! {
        <div
            data-name="CommandPalette"
            data-open=move || open.get().then_some("")
            class=move || {
                if open.get() {
                    "fixed inset-0 z-[80] flex items-start justify-center bg-black/60 p-4 pt-[15vh] opacity-100 transition-opacity"
                } else {
                    "fixed inset-0 z-[80] flex items-start justify-center bg-black/60 p-4 pt-[15vh] opacity-0 pointer-events-none transition-opacity"
                }
            }
            on:click=move |_| open.set(false)
        >
            <div
                role="dialog"
                aria-modal="true"
                aria-label="Command palette"
                // Closed = inert — the hidden card can't take focus.
                inert=move || (!open.get()).then_some("")
                aria-hidden=move || (!open.get()).then_some("true")
                class="flex w-full max-w-lg flex-col overflow-hidden rounded-xl border bg-popover shadow-xl"
                on:click=move |ev| ev.stop_propagation()
                // The keydown cascade: handled here (inside the card)
                // and stopped, so the document-level listeners in the
                // shell/session list never see palette keys.
                on:keydown=move |ev| {
                    match ev.key().as_str() {
                        "Escape" => {
                            ev.stop_propagation();
                            ev.prevent_default();
                            open.set(false);
                        }
                        "ArrowDown" | "ArrowUp" => {
                            ev.stop_propagation();
                            ev.prevent_default();
                            let len = ranked().len();
                            let dir = if ev.key() == "ArrowDown" {
                                NavDir::Down
                            } else {
                                NavDir::Up
                            };
                            if let Some(next) =
                                keymap::nav_index(Some(active.get_untracked()), len, dir)
                            {
                                active.set(next);
                            }
                        }
                        "Enter" => {
                            ev.stop_propagation();
                            ev.prevent_default();
                            let rows = ranked();
                            let i = active.get_untracked().min(rows.len().saturating_sub(1));
                            if let Some(item) = rows.get(i) {
                                activate(item);
                            }
                        }
                        _ => {}
                    }
                }
            >
                <div class="border-b p-2">
                    <Input
                        {..}
                        node_ref=input_ref
                        attr:r#type="text"
                        attr:role="combobox"
                        attr:aria-expanded="true"
                        attr:aria-controls="sepia-palette-list"
                        attr:aria-autocomplete="list"
                        attr:aria-activedescendant=move || {
                            format!("sepia-palette-opt-{}", active.get())
                        }
                        attr:placeholder="Type a command or search sessions…"
                        prop:value=move || query.get()
                        on:input=move |ev| {
                            query.set(event_target_value(&ev));
                            active.set(0);
                        }
                    />
                </div>
                <ul
                    id="sepia-palette-list"
                    role="listbox"
                    aria-label="Results"
                    class="max-h-80 overflow-y-auto p-1.5"
                >
                    {move || {
                        // Items exist only while open — SSR and first
                        // hydration both render the closed palette
                        // (empty listbox), so the sessions read never
                        // runs outside a Suspense boundary.
                        if !open.get() {
                            return ().into_any();
                        }
                        let rows = ranked();
                        if rows.is_empty() {
                            return view! {
                                <li class="px-3 py-6 text-center text-sm text-muted-foreground">
                                    "No results."
                                </li>
                            }
                            .into_any();
                        }
                        let act = active.get().min(rows.len() - 1);
                        rows
                            .into_iter()
                            .enumerate()
                            .map(|(i, item)| {
                                let selected_row = i == act;
                                let icon = row_icon(&item);
                                view! {
                                    <li
                                        id=format!("sepia-palette-opt-{i}")
                                        role="option"
                                        aria-selected=selected_row
                                        data-active=selected_row.then_some("")
                                        class=move || {
                                            if selected_row {
                                                "flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-sm bg-accent"
                                            } else {
                                                "flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-sm"
                                            }
                                        }
                                        on:mouseenter=move |_| active.set(i)
                                        on:click={
                                            let item = item.clone();
                                            move |_| activate(&item)
                                        }
                                    >
                                        <Icon name=icon class="text-muted-foreground"/>
                                        <span class="min-w-0 flex-1 truncate">{item.title.clone()}</span>
                                        <span class="shrink-0 truncate text-xs text-muted-foreground">
                                            {item.hint.clone()}
                                        </span>
                                    </li>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </ul>
                <div class="border-t px-3 py-1.5 text-[11px] text-muted-foreground">
                    "↑↓ navigate · Enter select · Esc close"
                </div>
            </div>
        </div>
    }
}
