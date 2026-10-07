//! Sepia node — Rust port of the Bun `apps/server` API surface.
//!
//! Layout mirrors the TS structure so the port stays navigable:
//! `env` = env.ts config validation, `app` = route registration,
//! `node` = node identity, `meta` = meta-overlay store, `events` = the
//! `/api/events` feed, `sessions`/`projects`/`servers` = route groups,
//! `acp` = agent runtime spawning (sepia-acp port), `store` = sepia-core
//! adapters (Devin sqlite, Cline dirs, Claude jsonl, Cursor store.db).

pub mod app;
pub mod env;
pub mod error;
pub mod node;

pub use error::{Error, Result};
