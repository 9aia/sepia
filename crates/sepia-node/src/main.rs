//! sepia-node — headless daemon binary. Discovers drivers, builds the
//! merged store + control plane, then (once `sepia-http` lands) serves
//! the API. Until then it prints a discovery summary and exits.

use sepia_control::ControlPlaneOptions;
use sepia_node::{NodePaths, build, sepia_home};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let paths = NodePaths::new(sepia_home());
    let node = build(&paths, ControlPlaneOptions::default()).await?;

    let drivers: Vec<String> = node.registry.manifests().map(|m| m.id.clone()).collect();
    let sessions = node.repo.list().await.unwrap_or_default();
    println!(
        "sepia-node: {} driver(s) [{}], {} session(s) visible",
        drivers.len(),
        drivers.join(", "),
        sessions.len()
    );
    node.plane.close_all().await;
    Ok(())
}
