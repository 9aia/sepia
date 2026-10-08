//! `App` — the shell + router. Compiles for both `ssr` (rendered to
//! HTML by `sepia-hub`) and `hydrate` (wasm interactivity).

use leptos::prelude::*;
#[cfg(feature = "ssr")]
use leptos_meta::MetaTags;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;

use crate::pages::{
    AgentsPage, NodesPage, ProjectsPage, SessionDetailPage, SessionListPage, SettingsPage,
};

/// Wall-clock ticker for relative-time labels — refreshed every 30s on
/// the client; a fixed snapshot during SSR.
#[derive(Clone, Copy)]
pub struct Now(pub RwSignal<f64>);

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let now = RwSignal::new(crate::time::now_ms());
    provide_context(Now(now));
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        let tick = Closure::<dyn FnMut()>::new(move || now.set(crate::time::now_ms()));
        if let Some(window) = web_sys::window() {
            let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
                tick.as_ref().unchecked_ref(),
                30_000,
            );
        }
        // App-lifetime ticker — leaking the closure keeps it alive.
        tick.forget();
    }
    view! {
        <Title text="sepia"/>
        <Router>
            <div class="app">
                <header class="topbar">
                    <A href="/" attr:class="brand">"sepia"</A>
                    <nav class="nav">
                        <A href="/" exact=true>"Sessions"</A>
                        <A href="/agents">"Agents"</A>
                        <A href="/projects">"Projects"</A>
                        <A href="/nodes">"Nodes"</A>
                        <A href="/settings">"Settings"</A>
                    </nav>
                </header>
                <main class="content">
                    <Routes fallback=|| {
                        view! { <p class="empty">"That page doesn't exist."</p> }
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
