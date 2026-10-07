use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use sepia_server::app::{AppState, build_app};
use sepia_server::env::parse_env;
use sepia_server::node::load_node_identity;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sepia_server=info".into()),
        )
        .json()
        .init();

    let env = match parse_env() {
        Ok(env) => env,
        Err(message) => {
            eprintln!("sepia-server: {message}");
            std::process::exit(1);
        }
    };

    // Non-loopback binds require a token — same rule as the Bun server.
    let loopback = matches!(env.host.as_str(), "127.0.0.1" | "localhost" | "::1")
        || env.host.starts_with("127.");
    if !loopback && env.token.is_none() {
        eprintln!("sepia-server: SEPIA_HOST={} requires SEPIA_TOKEN", env.host);
        std::process::exit(1);
    }

    let node = load_node_identity(&env.node_path, &env.node_name);
    let state = AppState {
        env: Arc::new(env.clone()),
        node: Arc::new(Mutex::new(node)),
    };
    let app = build_app(state);

    let addr: SocketAddr = match format!("{}:{}", env.host, env.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!(
                "sepia-server: invalid listen address {}:{} — {e}",
                env.host, env.port
            );
            std::process::exit(1);
        }
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("sepia-server: bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("sepia-server listening on http://{addr}/ (API only — no UI bundle)");
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    {
        eprintln!("sepia-server: serve: {e}");
        std::process::exit(1);
    }
}
