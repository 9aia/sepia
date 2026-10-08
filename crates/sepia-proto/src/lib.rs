//! sepia-proto — protocol v2 wire types shared by node, hub, web, and
//! testkit. Changing the protocol means changing this crate.

pub mod events;

pub use events::SessionEvent;
