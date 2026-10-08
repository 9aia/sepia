//! sepia-driver-cline — the Cline `~/.cline/data` adapter as a driver
//! binary. Ports `packages/cline/src/`:
//!
//! - [`cline`] — session-dir parsing → IR plus the manifest/transcript
//!   writers (`Cline.ts`).
//! - [`cline_index`] — the `db/sessions.db` index row shape the Cline CLI
//!   reads (`ClineIndex.ts`).
//! - [`config`] — workspace `.clinerules`/`cline_mcp_settings.json` reads
//!   and writes (`ClineConfig.ts`).
//! - [`store`] — the `SessionRepository` over the session dirs
//!   (`ClineRepository.ts`), plus the in-place transcript truncation.

pub mod cline;
pub mod cline_index;
pub mod config;
pub mod store;

pub use store::ClineStore;
