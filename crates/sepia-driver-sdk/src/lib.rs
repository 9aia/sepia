//! sepia-driver-sdk — the SDK for writing sepia store drivers.
//!
//! A driver is a standalone binary (`sepia-driver-<name>`) that speaks
//! ndjson JSON-RPC over stdio. `--manifest` prints the
//! [`manifest::DriverManifest`] and exits; with no args it runs the
//! [`serve`] loop answering `session.*`/`file.*` methods.

pub mod fs;
pub mod manifest;
pub mod methods;
pub mod rpc;
pub mod serve;

pub use manifest::{Capability, DRIVER_PROTOCOL, DriverManifest};
pub use serve::{Driver, SessionTruncator, StoreDriver, serve, serve_store};
