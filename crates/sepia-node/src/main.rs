//! sepia-node — the headless daemon. Discovers drivers, builds the
//! merged store + control plane, serves the `/api/*` surface, and shuts
//! the plane down on SIGINT/SIGTERM.

use std::sync::Arc;

use sepia_control::{ControlError, ControlPlaneOptions};
use sepia_http::AppState;
use sepia_http::env::Env;
use sepia_http::routes::sessions::ImportTarget;
use sepia_node::{NodePaths, build};

fn control_err(e: impl std::fmt::Display) -> ControlError {
    ControlError {
        code: sepia_control::ControlErrorCode::Internal,
        message: e.to_string(),
        cause: Some(e.to_string()),
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let env = Env::parse().map_err(std::io::Error::other)?;
    let paths = NodePaths::new(env.home.clone());
    let node = build(
        &paths,
        ControlPlaneOptions {
            idle_ttl: Some(env.idle_ttl),
            sweep_interval: Some(env.sweep),
            probe_cwd: std::env::current_dir().ok(),
            file_history_dir: Some(env.claude_dir.join("file-history")),
            terminate_lock_holder: Some(Arc::new(|pid| {
                // SIGTERM — the documented takeover signal.
                #[cfg(unix)]
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
                #[cfg(not(unix))]
                let _ = pid;
            })),
            ..ControlPlaneOptions::default()
        },
    )
    .await?;

    // Conversion seams — devin↔cline, like the TS `deps.convert`.
    let devin_store = node
        .registry
        .merged_store()
        .await
        .for_agent("devin")
        .map(|s| Arc::new(s.clone()) as Arc<dyn sepia_core::storage::SessionRepository>);
    let cline_dir = env.cline_dir.clone();
    if let Some(devin) = &devin_store {
        let repo = Arc::clone(devin);
        let cline = cline_dir.clone();
        let convert = Arc::new(move |id: String, target: ImportTarget| {
            let repo = Arc::clone(&repo);
            let cline = cline.clone();
            Box::pin(async move {
                match target {
                    ImportTarget::Cline => {
                        sepia_convert::install_cline(&repo, &id, &cline, None, false)
                            .await
                            .map_err(|e| control_err(e.message))
                    }
                    ImportTarget::Devin => sepia_convert::import_cline(
                        &cline.join("sessions").join(&id),
                        None,
                        &repo,
                        false,
                    )
                    .await
                    .map_err(|e| control_err(e.message)),
                }
            }) as futures::future::BoxFuture<'static, Result<String, ControlError>>
        }) as sepia_http::ConvertSession;
        let repo2 = Arc::clone(devin);
        let import = Arc::new(move |session: sepia_core::Session, _target: ImportTarget| {
            let repo = Arc::clone(&repo2);
            Box::pin(async move {
                sepia_convert::import_session(&repo, &session)
                    .await
                    .map_err(|e| control_err(e.message))
            }) as futures::future::BoxFuture<'static, Result<String, ControlError>>
        }) as sepia_http::ImportSession;
        let state = AppState::from_env(&env, Arc::clone(&node.plane))
            .with_convert(convert)
            .with_import_session(import);
        run(&env, state, node).await
    } else {
        run(
            &env,
            AppState::from_env(&env, Arc::clone(&node.plane)),
            node,
        )
        .await
    }
}

async fn run(env: &Env, state: AppState, node: sepia_node::Node) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind((env.host.as_str(), env.port)).await?;
    tracing::info!("sepia-node listening on {}:{}", env.host, env.port);
    let app = sepia_http::app(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("sepia-node shutting down");
            node.plane.close_all().await;
        })
        .await
}
