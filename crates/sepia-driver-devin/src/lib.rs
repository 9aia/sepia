//! sepia-driver-devin — Devin `sessions.db` adapter as a driver binary.

pub mod mapping;
pub mod store;

pub use store::DevinStore;
