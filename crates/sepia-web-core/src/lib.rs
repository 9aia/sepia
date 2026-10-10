//! sepia-web-core — the testable half of `sepia-web`, split out so
//! `cargo test -p sepia-web-core` runs natively in milliseconds. No
//! DOM, no wasm-bindgen, no leptos: plain data in, plain data out. The
//! Leptos components keep only event/view wiring and call into these
//! modules for every decision.

pub mod filter;
pub mod history;
pub mod keymap;
pub mod palette;
pub mod path;
pub mod theme;
pub mod transcript;
