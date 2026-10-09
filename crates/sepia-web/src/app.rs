//! `App` — the shell + router. Compiles for both `ssr` (rendered to
//! HTML by `sepia-hub`) and `hydrate` (wasm interactivity).
//!
//! Layout: a persistent left sidebar (brand + nav) on `lg+`; a slim
//! topbar with a hamburger opening a left `Sheet` on smaller screens.

use leptos::prelude::*;
#[cfg(feature = "ssr")]
use leptos_meta::MetaTags;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;
use tw_merge::IntoTailwindClass;

use crate::components::toast::{Toaster, provide_toaster};
use crate::components::{
    ButtonClass, ButtonSize, ButtonVariant, Sheet, SheetBody, SheetHeader, SheetTitle,
};
use crate::pages::{
    AgentsPage, NodesPage, ProjectsPage, SessionDetailPage, SessionListPage, SettingsPage,
};
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
                "Sessions"
            </A>
            <A href="/agents" attr:class=nav_link("w-full")>
                "Agents"
            </A>
            <A href="/projects" attr:class=nav_link("w-full")>
                "Projects"
            </A>
            <A href="/nodes" attr:class=nav_link("w-full")>
                "Nodes"
            </A>
            <A href="/settings" attr:class=nav_link("w-full")>
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
    let now = RwSignal::new(crate::time::now_ms());
    provide_context(Now(now));
    #[cfg(feature = "hydrate")]
    every_ms(30_000, move || now.set(crate::time::now_ms()));

    let sidebar_open = RwSignal::new(false);

    view! {
        <Title text="sepia"/>
        <Router>
            <div class="flex min-h-dvh">
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
                    <main class="flex min-w-0 flex-1 flex-col">
                        <Routes fallback=|| {
                            view! {
                                <p class="p-6 text-sm text-muted-foreground">
                                    "That page doesn't exist."
                                </p>
                            }
                        }>
                            <Route path=path!("/") view=SessionListPage/>
                            <Route path=path!("/sessions/:id") view=SessionDetailPage/>
                            <Route path=path!("/agents") view=AgentsPage/>
                            <Route path=path!("/projects") view=ProjectsPage/>
                            <Route path=path!("/nodes") view=NodesPage/>
                            <Route path=path!("/settings") view=SettingsPage/>
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
                <Toaster/>
            </div>
        </Router>
    }
}

/// The HTML document shell for SSR — head tags, hydration bootstrap,
/// and the stylesheet.
#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en" class="dark">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="theme-color" content="#1e1e2e"/>
                <title>"sepia"</title>
                <link rel="manifest" href="/manifest.json"/>
                <link rel="icon" href="/icon.svg" type="image/svg+xml"/>
                <link rel="stylesheet" href="/style.css"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options=options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}
