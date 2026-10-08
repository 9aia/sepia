//! sepia-web — the Leptos UI. Shared view code (`app`, pages, live
//! transcript, markdown-lite) compiles for both targets; the `ssr`
//! feature adds server functions, the `NodeApi` port, and the HTTP
//! client used by `sepia-hub`; the `hydrate` feature adds the wasm
//! entry point plus the `EventSource` glue.

pub mod api;
pub mod app;
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
