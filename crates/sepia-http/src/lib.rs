//! sepia-http — the axum driving adapter for the Sepia API. Port of
//! `apps/server/src/app.ts`: mounts `/api/*` over a `ControlPlane`, the
//! `MetaStore` overlay, and the node event feed. The `sepia-node`
//! daemon composes [`AppState`] and serves [`app`].

pub mod auth;
pub mod env;
pub mod feed;
pub mod node;
pub mod pair;
pub mod routes;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Router, serve as axum_serve};
use sepia_control::ControlPlane;
use sepia_meta::MetaStore;

pub use env::{Env, EnvError};
pub use feed::{EventFeed, HeldWatches, InstrumentedMeta, LiveListeners};
pub use node::{NodeIdentity, PROTOCOL_VERSION, SEPIA_VERSION, load_node_identity};
pub use pair::{Pairing, PairingStore};
pub use routes::sessions::{ConvertSession, ImportSession, ImportTarget};

/// Shared state threaded through every handler — the `createApp`
/// options object, minus the seams this crate doesn't port (transfer,
/// SSH tunnels, servers registry, push, telemetry, UI assets, AG-UI).
#[derive(Clone)]
pub struct AppState {
    pub plane: Arc<ControlPlane>,
    /// The instrumented meta overlay — `None` → meta routes answer 501.
    pub meta: Option<InstrumentedMeta>,
    /// Process-wide node event feed behind `GET /api/events`.
    pub feed: EventFeed,
    /// Identity reported by `GET /api/node`.
    pub node: NodeIdentity,
    /// Pairing backend for `POST /api/pair` + issued-credential auth.
    pub pairing: Option<Pairing>,
    /// `SEPIA_TOKEN` — `None`/empty means bearer auth is off entirely.
    pub token: Option<String>,
    /// CORS allowlist — `*` echoes any Origin back (bearer auth, not
    /// CORS, is the gate).
    pub origins: Vec<String>,
    /// SSE keep-alive cadence; `0` disables.
    pub keep_alive: Duration,
    /// Held-session watch state (re-probe cadence inside).
    pub held: HeldWatches,
    /// Per-session live listeners emitting `busy` feed events.
    pub live: LiveListeners,
    /// Store-write seam for `POST /api/sessions/import`.
    pub import_session: Option<ImportSession>,
    /// Store-write seam for `POST /api/sessions/:id/convert`.
    pub convert: Option<ConvertSession>,
    /// Web Push service over the meta store — `None` → 501s.
    pub push: Option<Arc<sepia_push::PushStore>>,
}

impl AppState {
    /// Minimal state — tokenless, no meta store, ephemeral node id.
    pub fn new(plane: Arc<ControlPlane>) -> Self {
        Self {
            plane,
            meta: None,
            feed: EventFeed::new(),
            node: NodeIdentity {
                id: format!("node_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
                name: hostname_or_unknown(),
                version: SEPIA_VERSION.to_string(),
            },
            pairing: None,
            token: None,
            origins: vec![
                "http://localhost:3000".to_string(),
                "http://127.0.0.1:3000".to_string(),
            ],
            keep_alive: Duration::from_millis(env::DEFAULT_SSE_KEEPALIVE_MS),
            held: HeldWatches::new(Duration::from_millis(env::DEFAULT_HELD_WATCH_MS)),
            live: LiveListeners::default(),
            import_session: None,
            convert: None,
            push: None,
        }
    }

    /// Assemble state from a parsed [`Env`] — the `serve.ts` wiring:
    /// meta store at `SEPIA_META`, node identity at `SEPIA_NODE`,
    /// pairing at `<home>/pair-code` + `<home>/tokens.json`.
    pub fn from_env(env: &Env, plane: Arc<ControlPlane>) -> Self {
        let feed = EventFeed::new();
        let meta = MetaStore::open(&env.meta_path);
        Self {
            plane,
            meta: Some(InstrumentedMeta::new(meta, feed.clone())),
            feed,
            node: load_node_identity(&env.node_path, &env.node_name),
            pairing: Some(Pairing::open(
                env.pair_code_path.clone(),
                env.tokens_path.clone(),
            )),
            token: env.token.clone(),
            origins: env.origins.clone(),
            keep_alive: env.sse_keep_alive,
            held: HeldWatches::new(env.held_watch),
            live: LiveListeners::default(),
            import_session: None,
            convert: None,
            push: Some(Arc::new(sepia_push::PushStore::open(MetaStore::open(
                &env.meta_path,
            )))),
        }
    }

    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    #[must_use]
    pub fn with_meta(mut self, meta: MetaStore) -> Self {
        self.meta = Some(InstrumentedMeta::new(meta, self.feed.clone()));
        self
    }

    #[must_use]
    pub fn with_pairing(mut self, pairing: Pairing) -> Self {
        self.pairing = Some(pairing);
        self
    }

    #[must_use]
    pub fn with_node(mut self, node: NodeIdentity) -> Self {
        self.node = node;
        self
    }

    #[must_use]
    pub fn with_origins(mut self, origins: Vec<String>) -> Self {
        self.origins = origins;
        self
    }

    #[must_use]
    pub fn with_keep_alive(mut self, keep_alive: Duration) -> Self {
        self.keep_alive = keep_alive;
        self
    }

    #[must_use]
    pub fn with_held_watch(mut self, interval: Duration) -> Self {
        self.held = HeldWatches::new(interval);
        self
    }

    #[must_use]
    pub fn with_import_session(mut self, f: ImportSession) -> Self {
        self.import_session = Some(f);
        self
    }

    /// Override the push service (tests wire a stub store).
    #[must_use]
    pub fn with_push(mut self, push: Option<Arc<sepia_push::PushStore>>) -> Self {
        self.push = push;
        self
    }

    #[must_use]
    pub fn with_convert(mut self, f: ConvertSession) -> Self {
        self.convert = Some(f);
        self
    }
}

fn hostname_or_unknown() -> String {
    nix::unistd::gethostname()
        .ok()
        .map(|h| h.to_string_lossy().into_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// CORS — port of `corsHeaders` + the OPTIONS preflight: `vary: Origin`
/// on every response, and the allow-* triple only when the request's
/// Origin is in the allowlist (or the allowlist carries `*`).
async fn cors_layer(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let allowed: HashSet<&str> = state.origins.iter().map(String::as_str).collect();
    let mut res = if req.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    let headers = res.headers_mut();
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = origin {
        if allowed.contains(origin.as_str()) || allowed.contains("*") {
            if let Ok(v) = HeaderValue::from_str(&origin) {
                headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
            }
            // PATCH/DELETE cover session meta, projects, config and
            // deletes for remote (federated) callers.
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET,POST,PATCH,DELETE,OPTIONS"),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("content-type,authorization"),
            );
        }
    }
    res
}

/// One line per request — `/api/health` excluded, like the TS logger.
async fn log_layer(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let started = Instant::now();
    let res = next.run(req).await;
    if path != "/api/health" {
        tracing::info!(
            "{} {} {} {}ms",
            method,
            path,
            res.status().as_u16(),
            started.elapsed().as_millis()
        );
    }
    res
}

/// Build the axum router — every route is `any()` + an internal method
/// guard so wrong-method requests fall through to the TS 404 (rather
/// than axum's bare 405).
pub fn app(state: AppState) -> Router {
    // The held-session watch ticker — re-probes lock state for sessions
    // last seen read-only while the feed has listeners.
    state
        .held
        .spawn_ticker(Arc::clone(&state.plane), state.feed.clone());

    Router::new()
        // Pre-auth routes — the auth gate itself passes these through
        // (health for probes; pair/login are the credential bootstrap).
        .route("/api/health", any(routes::misc::health))
        .route("/api/pair", any(auth::pair))
        .route("/api/auth/login", any(auth::handle))
        .route("/api/auth/logout", any(auth::handle))
        .route("/api/user", any(routes::misc::user))
        .route("/api/node", any(routes::misc::node))
        .route("/api/fs", any(routes::misc::fs))
        .route("/api/agents", any(routes::agents::agents))
        .route("/api/events", any(routes::events::events))
        .route("/api/client/keypair", any(routes::client::keypair))
        .route("/api/config", any(routes::config::get))
        .route("/api/config/{key}", any(routes::config::set))
        .route("/api/projects", any(routes::projects::collection))
        .route("/api/projects/{id}", any(routes::projects::item))
        .route("/api/sessions", any(routes::sessions::collection))
        .route(
            "/api/sessions/import",
            any(routes::sessions::import_session),
        )
        .route("/api/sessions/{id}", any(routes::sessions::item))
        .route("/api/sessions/{id}/meta", any(routes::sessions::meta_alias))
        .route("/api/sessions/{id}/history", any(routes::sessions::history))
        .route(
            "/api/sessions/{id}/checkpoints",
            any(routes::sessions::checkpoints),
        )
        .route("/api/sessions/{id}/export", any(routes::sessions::export))
        .route(
            "/api/sessions/{id}/stream",
            any(routes::sessions::stream_events),
        )
        .route("/api/sessions/{id}/attach", any(routes::sessions::attach))
        .route("/api/sessions/{id}/detach", any(routes::sessions::detach))
        .route("/api/sessions/{id}/prompt", any(routes::sessions::prompt))
        .route("/api/sessions/{id}/cancel", any(routes::sessions::cancel))
        .route(
            "/api/sessions/{id}/permission",
            any(routes::sessions::permission),
        )
        .route(
            "/api/sessions/{id}/permissions",
            any(routes::sessions::permission),
        )
        .route("/api/sessions/{id}/restore", any(routes::sessions::restore))
        .route("/api/sessions/{id}/rewind", any(routes::sessions::rewind))
        .route("/api/sessions/{id}/convert", any(routes::sessions::convert))
        .route("/api/push/vapid", any(routes::push::vapid))
        .route("/api/push/subscribe", any(routes::push::subscribe))
        .fallback(async || routes::not_found())
        .layer(axum::extract::DefaultBodyLimit::disable())
        // Innermost → outermost: auth gate, request log, CORS.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_gate,
        ))
        .layer(middleware::from_fn(log_layer))
        .layer(middleware::from_fn_with_state(state.clone(), cors_layer))
        .with_state(state)
}

/// `startServer` — bind `env.host:env.port` and serve the app. The bind
/// guard already ran in [`Env::parse`]; callers wanting graceful
/// shutdown own the listener themselves.
///
/// # Errors
/// `std::io::Error` on bind/serve failure.
pub async fn serve(env: &Env, state: AppState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind((env.host.as_str(), env.port)).await?;
    tracing::info!("sepia-server listening on {}:{}", env.host, env.port);
    axum_serve(listener, app(state)).await
}
