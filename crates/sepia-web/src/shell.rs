//! The HTML document shell for SSR — head tags, hydration bootstrap,
//! and the stylesheet.
//!
//! The whole module is `#[cfg(feature = "ssr")]` (declared in
//! `lib.rs`): the hydrate build never renders a document shell, so no
//! markup divergence is possible. Keeping the cfg at the module
//! boundary — never next to `view!`/`impl IntoView` inside the file —
//! is what the `cargo xtask check` lint enforces.

use leptos::prelude::*;
use leptos_meta::MetaTags;

use crate::app::App;

/// The `<html>` document — SSR only (module is cfg-gated in lib.rs).
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
                // Pre-paint theme reconcile — SSR always emits
                // `class="dark"` and the hydrate Effect runs after
                // wasm boots, so without this a stored light/system
                // theme flashes dark on every reload (FOUC).
                <script>
                    "try{var t=localStorage.getItem('sepia-theme'),d=t?t==='dark':matchMedia('(prefers-color-scheme: dark)').matches,e=document.documentElement;e.classList.toggle('dark',d);e.classList.toggle('light',!d);e.style.colorScheme=d?'dark':'light'}catch(_){}"
                </script>
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
