//! sepia — the CLI's library surface. `src/main.rs` is the thin clap
//! front-end; every verb implementation lives here so integration tests
//! can drive the same code the binary runs.

pub mod api;
pub mod config_ops;
pub mod node_ops;
pub mod pair;
pub mod service;
pub mod store;

/// The version stamp — same value `/api/node` reports (`sepia-http`'s
/// `SEPIA_VERSION` is the same `CARGO_PKG_VERSION`).
pub const SEPIA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A command failure — printed to stderr, exits 1.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}
