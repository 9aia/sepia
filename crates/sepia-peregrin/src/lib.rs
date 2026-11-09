//! `sepia-peregrin` — sepia's first-class adapter to peregrin's entity
//! plane.
//!
//! Peregrin is a generic distributed orchestrator: it projects,
//! routes, queues, locks, and moves **entities** —
//! `{id, kind, indexDoc}` documents with namespaced ops
//! (`<kind>.<verb>`) and kind-shaped snapshots. It never names a
//! sepia type. Sepia sessions register on the plane as
//! `kind = "sepia-session"`: the `indexDoc` is the session's
//! [`sepia_session::SessionSummary`] projection, `sepia.<verb>` ops
//! carry session actions, and a snapshot is the session IR.
//!
//! This crate is sepia's adapter to the entity plane, both directions:
//!
//! - [`SepiaPeregrin`] — the *client*: it speaks the entity wire
//!   (`/api/entities*` on any node or hub) and presents sessions.
//!   Session-detail traffic (history, checkpoints, streams) stays on
//!   the session plane — `raw_get`/`raw_post` reach it.
//! - [`SepiaEntityPlane`] + [`SessionBackend`] — the *server*: a
//!   `hypha_http::EntityPlane` impl for `kind="sepia-session"` that a
//!   hypha node host registers via `HyphaState::with_kind`, serving
//!   entity docs, `sepia.<verb>` ops, and IR snapshots straight off
//!   `sepia_session::Local` — no route-handler re-dispatch, no host
//!   downcast.

mod client;
mod doc;
mod plane;

pub use client::{SepiaPeregrin, SepiaPeregrinError};
pub use doc::{ENTITY_KIND, entity_id_of, entity_to_summary, summary_to_entity};
pub use plane::{SepiaEntityPlane, SessionBackend};
pub use sepia_session::SessionSummary;
