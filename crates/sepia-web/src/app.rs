//! `App` — the shell + router. Compiles for both `ssr` (rendered to
//! HTML by `sepia-hub`) and `hydrate` (wasm interactivity).
//!
//! Layout: the shadcn sidebar-09 pair on `lg+` — a narrow icon rail
//! (`IconRail`) plus an expandable content panel the current section
//! owns (today only Sessions: `SessionListPanel` renders the session
//! list beside the rail). Below `lg` the rail is gone; a slim topbar
//! with a hamburger opens the nav in a left `Sheet`.

#[cfg(feature = "ssr")]
pub use crate::shell::shell;
use leptos::prelude::*;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::NavigateOptions;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::hooks::use_location;
#[cfg(feature = "hydrate")]
use leptos_router::hooks::use_navigate;
use leptos_router::path;
#[cfg(feature = "hydrate")]
use sepia_web_core::keymap::{self, KeyCtx, Mods};
use tw_merge::IntoTailwindClass;

use crate::components::icons::Icon;
use crate::components::toast::{Toaster, provide_toaster};
use crate::components::{
    ButtonClass, ButtonSize, ButtonVariant, EmptyState, Sheet, SheetBody, SheetHeader, SheetTitle,
};
use crate::pages::{
    AgentsPage, LoginPage, NodesPage, ProjectsPage, SessionDetailPage, SessionListPage,
    SettingsPage,
};
use crate::palette::CommandPalette;
use crate::theme::ThemeToggle;

/// Wall-clock ticker for relative-time labels — refreshed every 30s on
/// the client; a fixed snapshot during SSR.
#[derive(Clone, Copy)]
pub struct Now(pub RwSignal<f64>);

/// The sidebar-09 panel open flag — provided by `App`, toggled by the
/// rail's Sessions button, the panel's own collapse button, and ⌘B
/// (`Action::ToggleList`). Only Sessions has panel content today, so
/// `SessionListPanel` is the sole reader; `true` (open) on SSR and
/// first hydration — the persisted choice is adopted post-mount
/// (class-driven collapse, same pattern as the collapsed groups).
#[derive(Clone, Copy)]
pub struct PanelOpen(pub RwSignal<bool>);

/// localStorage key for the persisted panel flag (`Option<bool>` so
/// "never stored" maps to the open default, not `false`).
pub const PANEL_STORAGE_KEY: &str = "sepia-panel-open";

/// Provide `PanelOpen` + (hydrate only) the seed-then-persist Effect.
/// The signal starts `true` on both targets so SSR and the first
/// client render agree.
fn provide_panel() {
    let open = RwSignal::new(true);
    provide_context(PanelOpen(open));
    #[cfg(feature = "hydrate")]
    {
        let (stored, set_stored, _clear) = leptos_use::storage::use_local_storage::<
            Option<bool>,
            codee::string::JsonSerdeCodec,
        >(PANEL_STORAGE_KEY);
        let mut init = true;
        Effect::new(move |_| {
            let cur = open.get();
            if std::mem::replace(&mut init, false) {
                open.set(stored.get_untracked().unwrap_or(true));
            } else {
                set_stored.set(Some(cur));
            }
        });
    }
}

/// Page-scoped polling — `leptos_use::use_interval_fn` under the hood;
/// the interval stops when the calling owner unmounts. The callback is
/// FnMut, so wrap it: `Mutex` keeps the lib's `Fn` bound satisfied.
#[cfg(feature = "hydrate")]
pub fn every_ms(ms: i32, f: impl FnMut() + 'static) {
    let f = std::sync::Arc::new(std::sync::Mutex::new(f));
    leptos_use::use_interval_fn(
        move || {
            if let Ok(mut f) = f.lock() {
                f();
            }
        },
        u64::try_from(ms).unwrap_or(1_000),
    );
}

/// Is the keydown aimed at a text-entry element? input/textarea/
/// select/contenteditable swallow plain keys like `n` and the arrows.
#[cfg(feature = "hydrate")]
pub fn in_editable(ev: &web_sys::KeyboardEvent) -> bool {
    use wasm_bindgen::JsCast;
    let Some(el) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    else {
        return false;
    };
    if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
        if html.is_content_editable() {
            return true;
        }
    }
    matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
}

/// Ghost nav link classes — muted until hovered, foreground when the
/// route is active (`<A>` sets `aria-current="page"`).
fn nav_link(extra: &str) -> String {
    tw_merge::tw_merge!(
        ButtonClass {
            variant: ButtonVariant::Ghost,
            size: ButtonSize::Sm,
        }
        .to_class(),
        "justify-start text-muted-foreground hover:text-foreground aria-[current]:text-foreground aria-[current]:bg-accent",
        extra
    )
}

/// The nav links — shared between the desktop sidebar and the mobile
/// drawer (which also closes itself on click).
fn nav_items(extra: &str, on_nav: Option<std::sync::Arc<dyn Fn() + Send + Sync>>) -> impl IntoView {
    view! {
        <nav
            class=format!("flex min-w-0 flex-col gap-0.5 px-2 {extra}")
            on:click=move |_| {
                if let Some(f) = &on_nav {
                    f();
                }
            }
        >
            <A href="/" exact=true attr:class=nav_link("w-full")>
                <Icon name="message-square"/>
                "Sessions"
            </A>
            <A href="/agents" attr:class=nav_link("w-full")>
                <Icon name="bot"/>
                "Agents"
            </A>
            <A href="/projects" attr:class=nav_link("w-full")>
                <Icon name="folder"/>
                "Projects"
            </A>
            <A href="/nodes" attr:class=nav_link("w-full")>
                <Icon name="server"/>
                "Nodes"
            </A>
            <A href="/settings" attr:class=nav_link("w-full")>
                <Icon name="settings"/>
                "Settings"
            </A>
        </nav>
    }
}

/// Shared rail-button recipe — `size-9` icon cells, the sidebar token
/// set, `aria-current`/`data-active` swap on the accent pair.
const RAIL_ITEM: &str = "inline-flex size-9 items-center justify-center rounded-lg transition-colors \
                         focus-visible:ring-2 focus-visible:ring-sidebar-ring";

/// One rail entry as an `<A>` — the router sets `aria-current="page"`,
/// the `aria-[current]:` pair styles it.
fn rail_link(href: &'static str, label: &'static str, icon: &'static str) -> impl IntoView {
    view! {
        <A
            href=href
            attr:class=format!(
                "{RAIL_ITEM} text-sidebar-foreground/60 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground aria-[current]:bg-sidebar-accent aria-[current]:text-sidebar-accent-foreground"
            )
            attr:aria-label=label
            attr:title=label
        >
            <Icon name=icon class="size-5"/>
        </A>
    }
}

/// The sidebar-09 icon rail (`lg+` only — mobile keeps the topbar +
/// drawer). Sections sit in a `nav` landmark; `title` doubles as the
/// tooltip (no tooltip primitive in the toolkit).
///
/// Item behavior, per the sidebar-09 contract: the Sessions button
/// is the only section with panel content — on `/` or `/sessions/:id`
/// it collapses/expands `PanelOpen` (`aria-expanded` mirrors it);
/// from any other page it navigates to `/` and opens the panel. The
/// remaining items are plain links — their pages render full-width
/// next to the rail (the panel is page-owned, so it simply isn't
/// mounted there). The theme toggle moves up from the old sidebar's
/// footer into the rail's bottom slot.
#[component]
fn IconRail() -> impl IntoView {
    let panel = expect_context::<PanelOpen>();
    let location = use_location();
    // `/` and every `/sessions/:id` are the Sessions section.
    let on_sessions = move || {
        let p = location.pathname.get();
        p == "/" || p.starts_with("/sessions/")
    };
    // `StoredValue` keeps the navigate fn Copy-able into the click
    // handler; SSR never navigates (`new_local` would panic on a
    // foreign worker at cleanup).
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    #[cfg(not(feature = "hydrate"))]
    let navigate = StoredValue::new(|_: &str, _: NavigateOptions| {});
    // Click semantics, per sidebar-09: on a sessions route the rail
    // item toggles its panel; anywhere else it activates the section
    // (navigates to `/` and opens the panel).
    let sessions_click = move |_: leptos::ev::MouseEvent| {
        if on_sessions() {
            panel.0.update(|o| *o = !*o);
        } else {
            panel.0.set(true);
            navigate.with_value(|n| n("/", NavigateOptions::default()));
        }
    };

    view! {
        <nav
            aria-label="Primary"
            data-name="IconRail"
            class="hidden w-14 shrink-0 flex-col items-center border-r border-sidebar-border bg-sidebar py-3 text-sidebar-foreground lg:flex"
        >
            // Brand — compact lettermark, links home like the old
            // sidebar's wordmark did.
            <A
                href="/"
                exact=true
                attr:class="mb-3 inline-flex size-9 items-center justify-center rounded-lg text-lg font-bold tracking-tight text-primary"
                attr:title="sepia — sessions"
                attr:aria-label="sepia — sessions"
            >
                "S"
            </A>
            <div class="flex flex-col items-center gap-1">
                <button
                    type="button"
                    data-name="RailSessions"
                    aria-label="Sessions"
                    aria-controls="sessions-panel"
                    aria-expanded=move || panel.0.get().to_string()
                    attr:aria-current=move || on_sessions().then_some("page")
                    title="Sessions"
                    class=move || {
                        if on_sessions() {
                            format!("{RAIL_ITEM} bg-sidebar-accent text-sidebar-accent-foreground")
                        } else {
                            format!(
                                "{RAIL_ITEM} text-sidebar-foreground/60 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
                            )
                        }
                    }
                    on:click=sessions_click
                >
                    <Icon name="message-square" class="size-5"/>
                </button>
                {rail_link("/agents", "Agents", "bot")}
                {rail_link("/projects", "Projects", "folder")}
                {rail_link("/nodes", "Nodes", "server")}
                {rail_link("/settings", "Settings", "settings")}
            </div>
            <div class="mt-auto pt-2">
                <ThemeToggle/>
            </div>
        </nav>
    }
        // Self-erase — `App`'s view type stays shallow.
        .into_any()
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    provide_toaster();
    crate::theme::provide_theme();
    crate::keybinds::provide_keybinds();
    provide_panel();
    // Shared async cache — keyed queries dedupe across components and
    // survive remounts; mutations invalidate by scope fn.
    leptos_fetch::QueryClient::new().provide();
    let now = RwSignal::new(crate::time::now_ms());
    provide_context(Now(now));
    // The 30s `now` ticker lives in `ShellHotkeys` — all hydrate-only
    // wiring stays in one component (and the cfg-markup lint wants
    // cfg attrs nowhere near a `view!`).

    let sidebar_open = RwSignal::new(false);
    let help_open = RwSignal::new(false);
    let palette_open = RwSignal::new(false);

    view! {
        <Title text="sepia"/>
        <Router>
            <div class="flex h-dvh overflow-hidden">
                // Desktop navigation — the sidebar-09 icon rail; the
                // content panel it opens lives inside the routed page
                // (`SessionListPanel` owns the Sessions section's).
                <IconRail/>
                <div class="flex min-w-0 flex-1 flex-col">
                    // Mobile topbar + drawer
                    <header class="sticky top-0 z-40 flex h-12 items-center gap-2 border-b bg-card/80 px-3 backdrop-blur lg:hidden">
                        <button
                            type="button"
                            aria-label="Open navigation"
                            class="inline-flex size-9 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
                            on:click=move |_| sidebar_open.set(true)
                        >
                            <Icon name="menu" class="size-5"/>
                        </button>
                        <A href="/" attr:class="font-bold tracking-tight text-primary">
                            "sepia"
                        </A>
                        <span class="ml-auto">
                            <ThemeToggle/>
                        </span>
                    </header>
                    <main class="flex min-w-0 flex-1 flex-col overflow-y-auto">
                        <Routes fallback=|| {
                            view! {
                                <EmptyState
                                    icon="file-question-mark"
                                    title="Page not found"
                                    description="That page doesn't exist — the link may be stale or the session was deleted."
                                >
                                    <A
                                        href="/"
                                        attr:class=ButtonClass {
                                            variant: ButtonVariant::Secondary,
                                            size: ButtonSize::Sm,
                                        }
                                        .to_class()
                                    >
                                        "Back to sessions"
                                    </A>
                                </EmptyState>
                            }
                        }>
                            <Route path=path!("/") view=SessionListPage/>
                            <Route path=path!("/sessions/:id") view=SessionDetailPage/>
                            <Route path=path!("/agents") view=AgentsPage/>
                            <Route path=path!("/projects") view=ProjectsPage/>
                            <Route path=path!("/nodes") view=NodesPage/>
                            <Route path=path!("/settings") view=SettingsPage/>
                            // Pre-auth — the hub never gates /login;
                            // the page covers the shell chrome itself.
                            <Route path=path!("/login") view=LoginPage/>
                        </Routes>
                    </main>
                </div>
                <Sheet open=sidebar_open side="left" class="w-64">
                    <SheetHeader>
                        <SheetTitle>"sepia"</SheetTitle>
                    </SheetHeader>
                    <SheetBody class="p-0 pt-2">
                        {nav_items("", Some(std::sync::Arc::new(move || sidebar_open.set(false))))}
                    </SheetBody>
                </Sheet>
                // Inside `<Router>` so `use_navigate` resolves — the
                // ⌘, action needs it.
                <ShellHotkeys open=help_open palette=palette_open/>
                <Toaster/>
                <ShortcutsHelp open=help_open/>
                <CommandPalette open=palette_open help=help_open/>
            </div>
        </Router>
    }
}

/// The document-level hotkey listener + the `Now` ticker — rendered
/// inside `<Router>` so `use_navigate` resolves (its only markup is a
/// marker span). Owns the shell actions: `?` toggles the cheat-sheet,
/// `⌘K`/`/` the command palette, Escape closes both, `⌘,` opens
/// `/settings`. Page-scoped actions stay in the session-list listener.
#[component]
fn ShellHotkeys(open: RwSignal<bool>, palette: RwSignal<bool>) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        // The 30s wall-clock tick behind every relative-time label —
        // `Now` is provided by `App`, an ancestor owner.
        let now = expect_context::<Now>().0;
        every_ms(30_000, move || now.set(crate::time::now_ms()));
        let navigate = use_navigate();
        let keybinds = crate::keybinds::use_keybinds();
        // ⌘B — the sidebar-09 panel toggle; `SessionListPanel` reads
        // the flag (a no-op where no panel is mounted).
        let panel = expect_context::<PanelOpen>();
        let _k = leptos_use::use_event_listener(
            document(),
            leptos::ev::keydown,
            move |ev: leptos::ev::KeyboardEvent| {
                let mods = Mods {
                    ctrl: ev.ctrl_key(),
                    meta: ev.meta_key(),
                    alt: ev.alt_key(),
                    shift: ev.shift_key(),
                };
                let ctx = KeyCtx {
                    typing: in_editable(&ev),
                    help_open: open.get_untracked(),
                    ..KeyCtx::default()
                };
                match keybinds.resolve(&ev.key(), mods, ctx) {
                    Some(keymap::Action::ToggleHelp) => {
                        ev.prevent_default();
                        open.update(|o| *o = !*o);
                    }
                    Some(keymap::Action::CloseHelp) => open.set(false),
                    Some(keymap::Action::TogglePalette) => {
                        ev.prevent_default();
                        palette.update(|o| *o = !*o);
                    }
                    Some(keymap::Action::OpenSettings) => {
                        ev.prevent_default();
                        navigate("/settings", NavigateOptions::default());
                    }
                    Some(keymap::Action::ToggleList) => {
                        ev.prevent_default();
                        panel.0.update(|o| *o = !*o);
                    }
                    // A stray Escape with the palette open and focus
                    // outside the card still closes it (the card's own
                    // keydown handles the focused case first).
                    _ => {
                        if ev.key() == "Escape" && palette.get_untracked() {
                            palette.set(false);
                        }
                    }
                }
            },
        );
    }
    // Zero-size marker — the component owns no chrome, but the sheet's
    // open flag lands here too (keeps `open` live on both targets and
    // gives the e2e suite a DOM hook for the listener's existence).
    view! {
        <span
            class="hidden"
            data-name="ShellHotkeys"
            attr:data-help-open=move || open.get().then_some("")
            attr:data-palette-open=move || palette.get().then_some("")
        ></span>
    }
}

/// The `?` sheet's row order — the bindable actions in their old
/// visual sequence with the fixed Escape row spliced in at its
/// original spot.
const HELP_ORDER: &[Option<sepia_web_core::keymap::Action>] = &[
    Some(sepia_web_core::keymap::Action::FocusNewSession),
    Some(sepia_web_core::keymap::Action::TogglePalette),
    None, // the fixed Escape cascade row
    Some(sepia_web_core::keymap::Action::ToggleList),
    Some(sepia_web_core::keymap::Action::OpenSettings),
    Some(sepia_web_core::keymap::Action::NavNext),
    Some(sepia_web_core::keymap::Action::NavPrev),
    Some(sepia_web_core::keymap::Action::FoldGroup),
    Some(sepia_web_core::keymap::Action::UnfoldGroup),
    Some(sepia_web_core::keymap::Action::ToggleHelp),
];

/// `?` — the shortcut cheat-sheet. Always rendered, visibility via
/// class (SSR and hydrate agree on a closed dialog). It's a custom
/// overlay, not a `Sheet`, but it carries the same dialog semantics:
/// `role=dialog` + `aria-modal` + `aria-labelledby`, initial focus +
/// the shared Tab trap + return-focus (`track_overlay_focus`), and
/// `aria-hidden`/`inert` while closed so the faded-out copy stays out
/// of the a11y tree and the tab order. Escape closes via the shell's
/// `CloseHelp` hotkey.
///
/// Rows read the effective keymap (defaults + `sepia-keymap`
/// overrides) so the sheet always shows what will actually fire —
/// SSR renders defaults; overrides land post-hydrate.
#[component]
fn ShortcutsHelp(open: RwSignal<bool>) -> impl IntoView {
    let keybinds = crate::keybinds::use_keybinds();
    let panel = NodeRef::<leptos::html::Div>::new();
    crate::components::track_overlay_focus(open, panel);
    view! {
        <div
            data-name="ShortcutsHelp"
            data-open=move || open.get().then_some("")
            attr:aria-hidden=move || (!open.get()).then_some("true")
            class=move || {
                if open.get() {
                    "fixed inset-0 z-[80] grid place-items-center bg-black/30 p-4 opacity-100 transition-opacity supports-backdrop-filter:backdrop-blur-sm"
                } else {
                    "fixed inset-0 z-[80] grid place-items-center bg-black/30 p-4 opacity-0 pointer-events-none transition-opacity"
                }
            }
            on:click=move |_| open.set(false)
        >
            <div
                node_ref=panel
                role="dialog"
                aria-modal="true"
                aria-labelledby="shortcuts-help-title"
                tabindex="-1"
                inert=move || (!open.get()).then_some("")
                class="w-full max-w-sm rounded-3xl bg-popover p-5 shadow-xl ring-1 ring-border focus:outline-none"
                on:click=move |ev| ev.stop_propagation()
            >
                <div class="flex items-center justify-between">
                    <h3 id="shortcuts-help-title" class="text-sm font-semibold">"Keyboard shortcuts"</h3>
                    <button
                        type="button"
                        aria-label="Close"
                        class="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
                        on:click=move |_| open.set(false)
                    >
                        <Icon name="x" class="size-3.5"/>
                    </button>
                </div>
                <dl class="mt-3 space-y-1.5">
                    {move || {
                        HELP_ORDER
                        .iter()
                        .map(|row| {
                            let (keys, desc) = match row {
                                None => (
                                    "Esc".to_string(),
                                    "clear filter / close session".to_string(),
                                ),
                                Some(action) => (
                                    keybinds
                                        .keymap()
                                        .binding_label(*action)
                                        .unwrap_or_else(|| "disabled".to_string()),
                                    action.label().to_string(),
                                ),
                            };
                            view! {
                                <div class="flex items-center gap-3">
                                    <kbd class="rounded border bg-muted px-1.5 py-0.5 font-mono text-[11px]">
                                        {keys}
                                    </kbd>
                                    <dd class="text-xs text-muted-foreground">{desc}</dd>
                                </div>
                            }
                        })
                        .collect::<Vec<_>>()
                    }}
                </dl>
            </div>
        </div>
    }
}
