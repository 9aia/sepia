//! sepia-hub — the SSR host binary. Serves the `sepia-web` Leptos UI
//! and bridges browser traffic to the node daemon over HTTP
//! (`SEPIA_NODE_URL`).
//!
//! Env: `SEPIA_HUB_HOST`/`SEPIA_HUB_PORT` (or `SEPIA_PORT`),
//! `SEPIA_NODE_URL`, `SEPIA_NODE_TOKEN`, `SEPIA_SITE_ROOT`
//! (or `LEPTOS_SITE_ROOT`), `LEPTOS_ENV`/`SEPIA_HUB_ENV`.
//! `cargo-leptos` sets the `LEPTOS_*` vars when run via
//! `cargo leptos watch`.

use sepia_hub::{HubConfig, hub_state, router};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = HubConfig::from_env().map_err(std::io::Error::other)?;
    let addr = config.socket_addr().map_err(std::io::Error::other)?;
    let state = hub_state(&config);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        %addr,
        node = %config.node_url,
        site_root = %config.site_root,
        "sepia-hub listening"
    );
    axum::serve(listener, app).await
}
