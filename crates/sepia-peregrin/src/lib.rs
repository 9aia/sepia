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
//! This crate is the sepia-side client: it speaks the entity wire
//! (`/api/entities*` on any peregrin node or hub) and presents
//! sessions. Peregrin stays generic; session semantics live here.

mod client;
mod doc;

pub use client::{SepiaPeregrin, SepiaPeregrinError};
pub use doc::{ENTITY_KIND, entity_id_of, entity_to_summary, summary_to_entity};
pub use sepia_session::SessionSummary;
