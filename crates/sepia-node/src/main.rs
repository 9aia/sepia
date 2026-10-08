//! sepia-node — the headless daemon. Discovers drivers, builds the
//! merged store + control plane, serves the `/api/*` surface, and shuts
//! the plane down on SIGINT/SIGTERM.
//!
//! The boot sequence lives in [`sepia_node::serve`] — `sepia serve`
//! (crates/sepia-cli) runs the same path.

use sepia_http::env::Env;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let env = Env::parse().map_err(std::io::Error::other)?;
    sepia_node::serve(&env).await
}
