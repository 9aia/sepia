//! sepia-driver-host — the daemon side of the driver protocol: discovery,
//! spawn/supervise, capability registry, and `SessionRepository` impls
//! over subprocess drivers.

pub mod client;
pub mod discover;
pub mod registry;
pub mod store;

pub use client::DriverClient;
pub use registry::{DriverEntry, DriverRegistry, command_resolves, refresh_manifest};
pub use store::{MergedStore, RemoteStore};
