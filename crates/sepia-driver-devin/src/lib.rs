//! sepia-driver-devin — Devin `sessions.db` adapter as a driver binary.

pub mod config;
pub mod mapping;
pub mod store;
pub mod truncate;

pub use store::DevinStore;
