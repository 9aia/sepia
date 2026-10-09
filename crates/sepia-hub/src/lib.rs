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

pub mod auth;
pub mod proxy;
pub mod push_routes;
pub mod sync_api;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::middleware;
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
    /// `SEPIA_HUB_TOKEN` — the browser→hub gate. `None` → every route
    /// is open (loopback-only deployments).
    pub hub_token: Option<Arc<str>>,
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
            hub_token: None,
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
    /// `SEPIA_HUB_TOKEN` — browser→hub auth; required on non-loopback
    /// binds, honored as bearer header, `?token=` query, or the
    /// `sepia_hub` cookie.
    pub hub_token: Option<String>,
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
            hub_token: None,
            site_root: default_site_root(),
            dev: true,
        }
    }
}

/// Order: `$LEPTOS_SITE_ROOT`/`$SEPIA_SITE_ROOT` → the `cargo xtask
/// site` staging dir (`$SEPIA_HOME/site`) → `target/site` (dev build)
/// → the crate-local `public/` (assets only — no wasm bundle).
fn default_site_root() -> String {
    if let Ok(p) = std::env::var("LEPTOS_SITE_ROOT").or_else(|_| std::env::var("SEPIA_SITE_ROOT")) {
        return p;
    }
    let sepia_site = std::env::var_os("SEPIA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/sepia"))
        })
        .map(|h| h.join("site"));
    for p in [sepia_site, Some(std::path::PathBuf::from("target/site"))]
        .into_iter()
        .flatten()
    {
        if p.join("pkg").is_dir() {
            return p.to_string_lossy().into_owned();
        }
    }
    concat!(env!("CARGO_MANIFEST_DIR"), "/public").to_string()
}

/// Loopback binds may serve tokenless; anything else requires
/// `SEPIA_HUB_TOKEN` (mirrors `sepia_http::env::is_loopback_host`).
fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "::1" || host == "[::1]" || host.starts_with("127.")
}

impl HubConfig {
    /// Parse from env. `Err` on a malformed port/address or a
    /// non-loopback bind without `SEPIA_HUB_TOKEN` — fail fast.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Test seam — parse from a key/value map instead of `std::env`.
    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self, String> {
        Self::from_lookup(|key| vars.get(key).cloned())
    }

    /// Shared parser behind [`from_env`](Self::from_env).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut cfg = Self::default();
        if let Some(v) = get("SEPIA_HUB_HOST") {
            cfg.host = v;
        }
        if let Some(v) = get("SEPIA_HUB_PORT").or_else(|| get("SEPIA_PORT")) {
            cfg.port = v.parse::<u16>().map_err(|_| {
                format!("SEPIA_HUB_PORT/SEPIA_PORT must be a port number, got {v:?}")
            })?;
        }
        if let Some(v) = get("SEPIA_NODE_URL") {
            cfg.node_url = v.trim_end_matches('/').to_string();
        }
        cfg.node_token = get("SEPIA_NODE_TOKEN").filter(|t| !t.is_empty());
        cfg.hub_token = get("SEPIA_HUB_TOKEN").filter(|t| !t.is_empty());
        cfg.dev = !matches!(
            get("LEPTOS_ENV")
                .or_else(|| get("SEPIA_HUB_ENV"))
                .as_deref(),
            Some("prod" | "PROD" | "production")
        );
        // The node's bind guard, applied to the browser surface: a
        // wildcard/LAN bind with no hub token would expose the UI.
        if !is_loopback_host(&cfg.host) && cfg.hub_token.is_none() {
            return Err(format!(
                "sepia-hub refuses to bind {} without SEPIA_HUB_TOKEN. Set SEPIA_HUB_TOKEN or bind a loopback address (SEPIA_HUB_HOST=127.0.0.1).",
                cfg.host
            ));
        }
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
        // Credential bootstrap — validates `?token=`, plants the
        // `sepia_hub` cookie, redirects to `/`. Pre-auth by definition.
        .route("/login", get(auth::login))
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
        // Browser gate — a no-op until `hub_token` is set.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_gate,
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
    state.hub_token = config.hub_token.clone().map(Into::into);
    let router_api = Arc::clone(&api);
    state.session_router = Some(Arc::new(move |id| router_api.node_url_for(id, None)));
    Ok(state)
}

fn leptos_options(config: &HubConfig) -> LeptosOptions {
    // `xtask site` renames the bundle `sepia_web_<hash>.{js,bg.wasm}`;
    // discover the stem so SSR emits hashed URLs — a stale cached
    // bundle can never hydrate against a newer page.
    let output_name = std::fs::read_dir(std::path::Path::new(&config.site_root).join("pkg"))
        .ok()
        .into_iter()
        .flat_map(std::iter::Iterator::flatten)
        .filter_map(|f| {
            let n = f.file_name().to_string_lossy().into_owned();
            n.strip_suffix(".js")
                .filter(|stem| stem.starts_with("sepia_web_"))
                .map(str::to_owned)
        })
        .max()
        .unwrap_or_else(|| "sepia_web".to_string());
    LeptosOptions::builder()
        // wasm-bindgen names the bundle after the *lib* crate.
        .output_name(output_name)
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
