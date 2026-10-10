//! `App` — the shell + router. Compiles for both `ssr` (rendered to
//! HTML by `sepia-hub`) and `hydrate` (wasm interactivity).
//!
//! Layout: a persistent left sidebar (brand + nav) on `lg+`; a slim
//! topbar with a hamburger opening a left `Sheet` on smaller screens.

#[cfg(feature = "ssr")]
pub use crate::shell::shell;
use leptos::prelude::*;
use leptos_meta::{Title, provide_meta_context};
#[cfg(feature = "hydrate")]
use leptos_router::NavigateOptions;
use leptos_router::components::{A, Route, Router, Routes};
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
                <Icon name="sessions"/>
                "Sessions"
            </A>
            <A href="/agents" attr:class=nav_link("w-full")>
                <Icon name="agents"/>
                "Agents"
            </A>
            <A href="/projects" attr:class=nav_link("w-full")>
                <Icon name="projects"/>
                "Projects"
            </A>
            <A href="/nodes" attr:class=nav_link("w-full")>
                <Icon name="nodes"/>
                "Nodes"
            </A>
            <A href="/settings" attr:class=nav_link("w-full")>
                <Icon name="settings"/>
                "Settings"
            </A>
        </nav>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    provide_toaster();
    crate::theme::provide_theme();
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
                // Desktop sidebar — nav only; the sessions list itself
                // lives inside the `/` page (master-detail).
                <aside class="hidden w-56 shrink-0 flex-col border-r bg-card lg:flex">
                    <div class="px-4 pb-3 pt-4">
                        <A href="/" attr:class="text-lg font-bold tracking-tight text-primary">
                            "sepia"
                        </A>
                    </div>
                    {nav_items("", None)}
                    <div class="mt-auto flex items-center justify-between px-4 py-3">
                        <span class="text-xs text-muted-foreground">
                            "multi-node agent sessions"
                        </span>
                        <ThemeToggle/>
                    </div>
                </aside>
                <div class="flex min-w-0 flex-1 flex-col">
                    // Mobile topbar + drawer
                    <header class="sticky top-0 z-40 flex h-12 items-center gap-2 border-b bg-card/80 px-3 backdrop-blur lg:hidden">
                        <button
                            type="button"
                            aria-label="Open navigation"
                            class="inline-flex size-9 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
                            on:click=move |_| sidebar_open.set(true)
                        >
                            <svg class="size-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
                                <path d="M3 5h14M3 10h14M3 15h14" stroke-linecap="round"/>
                            </svg>
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
                                    icon="404"
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
                match keymap::resolve_key(&ev.key(), mods, ctx) {
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

/// The global shortcut list — rendered by the `?` cheat-sheet and,
/// read-only, by `/settings`.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("n", "focus the new-session field"),
    ("⌘K or /", "command palette"),
    ("Esc", "clear filter / close session"),
    ("⌘B", "toggle the session list"),
    ("⌘,", "open settings"),
    ("↑ / ↓", "cycle sessions"),
    ("← / →", "fold / unfold groups"),
    ("?", "this sheet"),
];

/// `?` — the shortcut cheat-sheet. Always rendered, visibility via
/// class (SSR and hydrate agree on a closed dialog).
#[component]
fn ShortcutsHelp(open: RwSignal<bool>) -> impl IntoView {
    let rows = SHORTCUTS;
    view! {
        <div
            data-name="ShortcutsHelp"
            data-open=move || open.get().then_some("")
            class=move || {
                if open.get() {
                    "fixed inset-0 z-[80] grid place-items-center bg-black/60 p-4 opacity-100 transition-opacity"
                } else {
                    "fixed inset-0 z-[80] grid place-items-center bg-black/60 p-4 opacity-0 pointer-events-none transition-opacity"
                }
            }
            on:click=move |_| open.set(false)
        >
            <div
                class="w-full max-w-sm rounded-xl border bg-popover p-5 shadow-xl"
                on:click=move |ev| ev.stop_propagation()
            >
                <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold">"Keyboard shortcuts"</h3>
                    <button
                        type="button"
                        class="text-xs text-muted-foreground hover:text-foreground"
                        on:click=move |_| open.set(false)
                    >
                        "✕"
                    </button>
                </div>
                <dl class="mt-3 space-y-1.5">
                    {rows
                        .iter()
                        .map(|(k, d)| {
                            view! {
                                <div class="flex items-center gap-3">
                                    <kbd class="rounded border bg-muted px-1.5 py-0.5 font-mono text-[11px]">
                                        {*k}
                                    </kbd>
                                    <dd class="text-xs text-muted-foreground">{*d}</dd>
                                </div>
                            }
                        })
                        .collect::<Vec<_>>()}
                </dl>
            </div>
        </div>
    }
}
