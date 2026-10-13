//! sepia-node — the headless daemon. Discovers drivers, builds the
//! merged store + control plane, serves the `/api/*` surface, and shuts
//! the plane down on SIGINT/SIGTERM.
//!
//! The boot sequence lives in [`sepia_node::serve`] — `sepia serve`
//! (crates/sepia-cli) runs the same path.

use sepia_http::env::Env;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // fmt + RUST_LOG as always; OTLP span export layered on when
    // SEPIA_OTEL_ENDPOINT / OTEL_EXPORTER_OTLP_* is set. The guard's
    // Drop flushes the provider on exit.
    let _telemetry = sepia_telemetry::init();

    let env = Env::parse().map_err(std::io::Error::other)?;
    sepia_node::serve(&env).await
}
