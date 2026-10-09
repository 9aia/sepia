//! sepia-driver-cline — the Cline `~/.cline/data` adapter as a driver
//! binary.
//!
//! - [`cline`] — session-dir parsing → IR plus the manifest/transcript
//!   writers.
//! - [`cline_index`] — the `db/sessions.db` index row shape the Cline CLI
//!   reads.
//! - [`config`] — workspace `.clinerules`/`cline_mcp_settings.json` reads
//!   and writes.
//! - [`store`] — the `SessionRepository` over the session dirs, plus the
//!   in-place transcript truncation.

pub mod cline;
pub mod cline_index;
pub mod config;
pub mod store;

pub use store::ClineStore;
