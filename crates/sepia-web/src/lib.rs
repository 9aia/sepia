//! sepia-web — the Leptos UI. Shared view code (`app`, pages, live
//! transcript, markdown-lite) compiles for both targets; the `ssr`
//! feature adds server functions, the `NodeApi` port, and the HTTP
//! client used by `sepia-hub`; the `hydrate` feature adds the wasm
//! entry point plus the `EventSource` glue.

pub mod api;
pub mod app;
pub mod components;
pub mod dto;
pub mod live;
pub mod markdown;
pub mod pages;
#[cfg(feature = "hydrate")]
pub mod sse;
pub mod time;

pub use app::App;

/// The hand-rolled stylesheet, embedded so the hub can serve it without
/// a cargo-leptos asset pipeline.
pub const STYLE_CSS: &str = include_str!("../style/main.css");

/// wasm-bindgen entry point — `cargo-leptos` (or a manual
/// `wasm-bindgen` run) emits a loader that calls this.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
    register_service_worker();
}

/// Best-effort PWA service-worker registration.
#[cfg(feature = "hydrate")]
fn register_service_worker() {
    let window = leptos::prelude::window();
    let _ = window.navigator().service_worker().register("/sw.js");
}

/// `setTimeout`-backed async sleep (wasm only).
#[cfg(feature = "hydrate")]
pub async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;

    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = leptos::prelude::window().set_timeout_with_callback_and_timeout_and_arguments_0(
            &resolve,
            i32::try_from(ms).unwrap_or(i32::MAX),
        );
    });
    let _ = JsFuture::from(promise).await;
}

/// Theme — light/dark/system. The tri-state lives in localStorage
/// (`sepia-theme`; absent = system); a hydrate-only Effect applies the
/// `dark`/`light` class to `<html>` and keeps it in sync with
/// `prefers-color-scheme` while in system mode.
pub mod theme {
    #[cfg(feature = "hydrate")]
    use leptos::prelude::*;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Theme {
        System,
        Dark,
        Light,
    }

    impl Theme {
        pub fn label(self) -> &'static str {
            match self {
                Self::System => "System",
                Self::Dark => "Dark",
                Self::Light => "Light",
            }
        }
        /// Cycle for the toggle button: dark → light → system → dark.
        #[must_use]
        pub fn next(self) -> Self {
            match self {
                Self::Dark => Self::Light,
                Self::Light => Self::System,
                Self::System => Self::Dark,
            }
        }
    }

    /// RwSignal-backed theme state; `Dark` on SSR (the shell emits
    /// `class="dark"`); hydrate-only Effect reconciles storage + media.
    #[cfg(feature = "hydrate")]
    pub fn provide_theme() -> RwSignal<Theme> {
        use codee::string::FromToStringCodec;
        use leptos_use::storage::use_local_storage;
        use leptos_use::use_media_query;

        let theme = RwSignal::new(Theme::Dark);
        provide_context(theme);

        // localStorage — `FromToStringCodec` round-trips the label str.
        let (stored, set_stored, _) = use_local_storage::<String, FromToStringCodec>("sepia-theme");
        let prefers_dark = use_media_query("(prefers-color-scheme: dark)");

        // Seed from storage once (SSR rendered `dark` unconditionally).
        let seeded = RwSignal::new(false);
        Effect::new(move |_| {
            let s = stored.get();
            if !seeded.get() {
                seeded.set(true);
                theme.set(match s.as_str() {
                    "light" => Theme::Light,
                    "dark" => Theme::Dark,
                    _ => Theme::System,
                });
            }
            let effective = match theme.get() {
                Theme::Dark => true,
                Theme::Light => false,
                Theme::System => prefers_dark.get(),
            };
            if let Some(doc) = document().document_element() {
                let _ = doc.class_list().toggle_with_force("dark", effective);
                let _ = doc.class_list().toggle_with_force("light", !effective);
                let _ = doc.set_attribute(
                    "style",
                    &format!("color-scheme: {}", if effective { "dark" } else { "light" }),
                );
            }
        });

        // Persist explicit choices; "system" clears the key.
        Effect::new(move |_| {
            if !seeded.get() {
                return;
            }
            match theme.get() {
                Theme::System => set_stored.set(String::new()),
                Theme::Dark => set_stored.set("dark".into()),
                Theme::Light => set_stored.set("light".into()),
            }
        });
        theme
    }

    /// Compact toggle button for the sidebar/topbar.
    #[cfg(feature = "hydrate")]
    #[component]
    pub fn ThemeToggle() -> impl IntoView {
        let theme = expect_context::<RwSignal<Theme>>();
        view! {
            <button
                type="button"
                title=move || format!("Theme: {} (click to cycle)", theme.get().label())
                class="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                on:click=move |_| theme.update(|t| *t = t.next())
            >
                {move || match theme.get() {
                    Theme::Dark => "☾",
                    Theme::Light => "☀",
                    Theme::System => "◐",
                }}
            </button>
        }
    }

    /// SSR stub — the shell always emits `class="dark"`; hydrate
    /// reconciles. Keeps `App` unconditional.
    #[cfg(not(feature = "hydrate"))]
    pub fn provide_theme() {}
    #[cfg(not(feature = "hydrate"))]
    #[leptos::prelude::component]
    pub fn ThemeToggle() -> impl leptos::prelude::IntoView {
        // Always render the button (SSR+hydrate must agree); it just
        // does nothing server-side.
        "☾"
    }
}
