//! sepia-hub — the SSR host. Serves the `sepia-web` Leptos UI and
//! bridges browser traffic to the node daemon:
//!
//! - `/`, `/sessions/{id}` — server-rendered Leptos routes.
//! - `/hub/*` — leptos server functions (auto-registered by
//!   [`LeptosRoutes`]).
//! - `/api/sessions/{id}/stream`, `/api/events` — SSE bridges to
//!   `SEPIA_NODE_URL` (EventSource can't carry cross-origin auth
//!   cleanly; proxying keeps the browser same-origin).
//! - `/api/*` — generic passthrough for the rest of the node API.
//! - everything else — static files from the site root (`manifest.json`,
//!   `sw.js`, `icon.svg`, `pkg/` when cargo-leptos has run).

pub mod proxy;
pub mod push_routes;
pub mod sync_api;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{any, get};
use leptos::config::LeptosOptions;
use leptos::prelude::provide_context;
use leptos_axum::{LeptosRoutes, file_and_error_handler_with_context, generate_route_list};
use sepia_web::api::NodeApi;
use sepia_web::app::{App, shell};

/// Everything the router needs — the leptos render options plus the
/// node-API port (`NodeApi` is a trait so tests can stub it) and the
/// upstream address for the HTTP/SSE bridges.
/// Session id → `(url, token)` of the owning node.
pub type SessionRouter = Arc<dyn Fn(&str) -> Option<(String, Option<String>)> + Send + Sync>;

#[derive(Clone)]
pub struct HubState {
    pub options: LeptosOptions,
    pub node: Arc<dyn NodeApi>,
    pub node_url: Arc<str>,
    pub node_token: Option<Arc<str>>,
    /// Multi-node routing — resolves a session id to its owning
    /// node's `(url, token)` for session-scoped proxy calls.
    /// `None` → everything uses `node_url` (single-node mode).
    pub session_router: Option<SessionRouter>,
    /// Hub-owned push subscription store — `None` → `/api/push/*` 501s.
    pub push: Option<Arc<sepia_push::PushStore>>,
    /// Long-lived streams: no global deadline, but a 30s body-recv
    /// timeout so a half-dead client is detected on the next ping.
    sse_agent: ureq::Agent,
    /// Bounded request/response passthroughs.
    http_agent: ureq::Agent,
}

impl HubState {
    /// Build the state from parts. `node_url` is the node's origin
    /// (`http://127.0.0.1:8787`); `node_token` is forwarded as a bearer
    /// token on hub → node calls, falling back to the browser's own
    /// `Authorization` header when unset.
    pub fn new(
        options: LeptosOptions,
        node: Arc<dyn NodeApi>,
        node_url: impl Into<Arc<str>>,
        node_token: Option<Arc<str>>,
    ) -> Self {
        let sse_agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(None)
                .timeout_connect(Some(std::time::Duration::from_secs(10)))
                .timeout_recv_body(Some(std::time::Duration::from_secs(30)))
                .build(),
        );
        let http_agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(60)))
                .build(),
        );
        Self {
            options,
            node,
            node_url: node_url.into(),
            node_token,
            session_router: None,
            push: None,
            sse_agent,
            http_agent,
        }
    }

    /// Route a session-scoped request: the owning node's url+token when
    /// a [`session_router`](Self::session_router) resolves, else the
    /// default upstream.
    pub(crate) fn session_upstream(&self, id: &str) -> (String, Option<String>) {
        self.session_router
            .as_ref()
            .and_then(|r| r(id))
            .unwrap_or_else(|| {
                (
                    self.node_url.to_string(),
                    self.node_token.as_ref().map(ToString::to_string),
                )
            })
    }

    /// Agent configured for open-ended SSE reads.
    pub(crate) fn sse_agent(&self) -> ureq::Agent {
        self.sse_agent.clone()
    }

    /// Agent configured for bounded JSON calls.
    pub(crate) fn json_agent(&self) -> ureq::Agent {
        self.http_agent.clone()
    }
}

impl FromRef<HubState> for LeptosOptions {
    fn from_ref(state: &HubState) -> Self {
        state.options.clone()
    }
}

/// Runtime configuration, all `SEPIA_*`/`LEPTOS_*` env.
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// `SEPIA_HUB_HOST` — default `127.0.0.1`.
    pub host: String,
    /// `SEPIA_HUB_PORT` — default `3000`.
    pub port: u16,
    /// `SEPIA_NODE_URL` — default `http://127.0.0.1:8787`.
    pub node_url: String,
    /// `SEPIA_NODE_TOKEN` — bearer token forwarded hub → node.
    pub node_token: Option<String>,
    /// `SEPIA_SITE_ROOT`/`LEPTOS_SITE_ROOT` — static assets dir.
    pub site_root: String,
    /// `LEPTOS_ENV`/`SEPIA_HUB_ENV` — `dev` or `prod`.
    pub dev: bool,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 3000,
            node_url: "http://127.0.0.1:8787".into(),
            node_token: None,
            site_root: default_site_root(),
            dev: true,
        }
    }
}

/// The crate-local `public/` dir when running from a checkout,
/// `target/site` after a `cargo-leptos` build, else `./public`.
fn default_site_root() -> String {
    if let Ok(p) = std::env::var("LEPTOS_SITE_ROOT").or_else(|_| std::env::var("SEPIA_SITE_ROOT")) {
        return p;
    }
    let bundled = concat!(env!("CARGO_MANIFEST_DIR"), "/public");
    if std::path::Path::new(bundled).is_dir() {
        bundled.to_string()
    } else {
        "public".to_string()
    }
}

impl HubConfig {
    /// Parse from env. `Err` on malformed port/address — fail fast.
    pub fn from_env() -> Result<Self, String> {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("SEPIA_HUB_HOST") {
            cfg.host = v;
        }
        if let Ok(v) = std::env::var("SEPIA_HUB_PORT").or_else(|_| std::env::var("SEPIA_PORT")) {
            cfg.port = v.parse::<u16>().map_err(|_| {
                format!("SEPIA_HUB_PORT/SEPIA_PORT must be a port number, got {v:?}")
            })?;
        }
        if let Ok(v) = std::env::var("SEPIA_NODE_URL") {
            cfg.node_url = v.trim_end_matches('/').to_string();
        }
        cfg.node_token = std::env::var("SEPIA_NODE_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        cfg.dev = !matches!(
            std::env::var("LEPTOS_ENV")
                .or_else(|_| std::env::var("SEPIA_HUB_ENV"))
                .as_deref(),
            Ok("prod" | "PROD" | "production")
        );
        Ok(cfg)
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, String> {
        format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|_| format!("invalid hub address {}:{}", self.host, self.port))
    }
}

/// Build the axum router. `HubConfig` → `HubState` happens in
/// [`hub_state`]; tests construct `HubState` directly with a stub
/// `NodeApi`.
pub fn router(state: HubState) -> Router {
    let routes = generate_route_list(App);
    let provide_node = {
        let node = Arc::clone(&state.node);
        move || provide_context::<Arc<dyn NodeApi>>(node.clone())
    };
    let shell_fn = {
        let options = state.options.clone();
        move || shell(options.clone())
    };
    Router::new()
        .route("/style.css", get(serve_css))
        // SSE bridges — must be registered before the `/api/*` wildcard.
        .route("/api/events", get(proxy::sse_events))
        .route("/api/sessions/{id}/stream", get(proxy::sse_session_stream))
        // Hub-owned push — intercepted before the passthrough so every
        // node notifies through one subscription store.
        .route("/api/push/vapid", any(push_routes::vapid))
        .route("/api/push/subscribe", any(push_routes::subscribe))
        .route("/api/{*rest}", any(proxy::passthrough))
        .leptos_routes_with_context(&state, routes, provide_node.clone(), shell_fn)
        .fallback(file_and_error_handler_with_context::<HubState, _>(
            provide_node,
            shell,
        ))
        .with_state(state)
}

/// HubState for a running hub: the sync engine + [`SyncNodeApi`].
/// `SEPIA_NODES` (`id=url[:token];…`) wins; `SEPIA_NODE_URL`/`SEPIA_NODE_TOKEN`
/// is the single-node shorthand. The projection + outbox live under
/// `$SEPIA_HOME/hub/` so they survive restarts.
///
/// # Errors
/// `String` on a missing/empty node list or a projection/outbox open failure.
pub fn hub_state(config: &HubConfig) -> Result<HubState, String> {
    let nodes = std::env::var("SEPIA_NODES")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|raw| sync_api::parse_nodes(&raw))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            vec![sync_api::HubNode {
                id: "node".to_string(),
                url: config.node_url.clone(),
                token: config.node_token.clone(),
            }]
        });
    let home = std::env::var_os("SEPIA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/sepia"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".sepia"))
        .join("hub");
    std::fs::create_dir_all(&home).map_err(|e| format!("create {}: {e}", home.display()))?;
    let projection = sepia_sync::ProjectionStore::open(&home.join("projection.db"))
        .map_err(|e| format!("projection open: {e}"))?;
    let outbox = sepia_outbox::Outbox::open(&home.join("outbox.db")).map_err(|e| e.to_string())?;
    let targets = nodes.iter().map(|n| {
        let node = sepia_sync::NodeRef {
            id: n.id.clone(),
            url: n.url.clone(),
            label: n.id.clone(),
        };
        match &n.token {
            Some(t) => sepia_sync::engine::NodeTarget::with_token(node, t.clone()),
            None => sepia_sync::engine::NodeTarget::new(node),
        }
    });
    let push = Arc::new(sepia_push::PushStore::open(sepia_meta::MetaStore::open(
        &home.join("push.json"),
    )));
    let projection_for_hook = projection.clone();
    let push_for_hook = Arc::clone(&push);
    let options = sepia_sync::engine::SyncOptions {
        on_event: Some(Arc::new(
            move |node_id: &str, event: &sepia_sync::client::FeedEvent| {
                let sepia_sync::client::FeedEvent::Diff {
                    kind, id, patch, ..
                } = event
                else {
                    return;
                };
                if kind != "session" {
                    return;
                }
                let title = projection_for_hook.session(node_id, id).map_or_else(
                    |_| id.clone(),
                    |opt| opt.map_or_else(|| id.clone(), |r| r.title),
                );
                let url = format!("/?session={id}");
                if patch.get("runFinished") == Some(&serde_json::Value::Bool(true)) {
                    push_for_hook.send(
                        sepia_push::Kind::Done,
                        "Session finished",
                        &format!("{title} finished its run."),
                        &url,
                    );
                }
                if patch.get("permissionRequested") == Some(&serde_json::Value::Bool(true)) {
                    push_for_hook.send(
                        sepia_push::Kind::Permission,
                        "Approval needed",
                        &format!("{title} is waiting for you."),
                        &url,
                    );
                }
            },
        ) as sepia_sync::engine::FeedHook),
        ..sepia_sync::engine::SyncOptions::default()
    };
    let engine = sepia_sync::SyncEngine::spawn(projection, outbox, targets, options);
    let options = leptos_options(config);
    let Some(upstream) = nodes.first().cloned() else {
        return Err("no nodes configured".into());
    };
    let api = Arc::new(sync_api::SyncNodeApi::new(engine, nodes));
    let mut state = HubState::new(
        options,
        Arc::clone(&api) as Arc<dyn NodeApi>,
        upstream.url.as_str(),
        upstream.token.clone().map(Into::into),
    );
    state.push = Some(push);
    let router_api = Arc::clone(&api);
    state.session_router = Some(Arc::new(move |id| router_api.node_url_for(id, None)));
    Ok(state)
}

fn leptos_options(config: &HubConfig) -> LeptosOptions {
    LeptosOptions::builder()
        // wasm-bindgen names the bundle after the *lib* crate.
        .output_name("sepia_web")
        .site_root(config.site_root.clone())
        .site_pkg_dir("pkg")
        .env(if config.dev {
            leptos::config::Env::DEV
        } else {
            leptos::config::Env::PROD
        })
        .build()
}

/// The embedded stylesheet — one source of truth in
/// `sepia-web/style/main.css`, no asset pipeline required.
async fn serve_css() -> impl axum::response::IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        sepia_web::STYLE_CSS,
    )
}
