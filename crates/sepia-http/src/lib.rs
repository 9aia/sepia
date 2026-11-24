//! sepia-http — the axum driving adapter for the session REST surface.
//! `/api/sessions*`, `/api/agents`, the `/api/config*` +
//! `/api/projects*` compatibility routes (canonical storage is the
//! node-local `hypha-kv`), the session event feed, and the meta/watch
//! side-channels all live here, composed over [`SessionState`] — the
//! [`SessionPlane`] port plus the host seams (meta overlay, KV, feed,
//! pairing, push, held/live bookkeeping). The generic substrate
//! routes (`/api/entities*`, `/api/events`, `/api/node`, `/api/pair`,
//! `/api/auth/*`, `/api/push/*`, `/api/kv*`, `/api/fs`, `/api/user`,
//! `/api/health`) are `hypha-http`'s; hosts merge
//! [`app_routes`] with `hypha_http::app_routes` under one listener
//! (peregrin's `compose` is the production merger).
//!
//! The `kind="sepia-session"` entity plane is [`SessionPlaneBackend`]
//! over this same state — entity ops keep the `/api/sessions*`
//! semantics (meta overlay, pending rows, feed/watch bookkeeping)
//! without re-entering the routes.

pub mod auth;
pub mod feed;
pub mod kv;
pub mod push;
pub mod routes;
pub mod session_backend;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use sepia_meta::MetaStore;
use sepia_session::{NodeIdentity, SessionPlane};
use tracing::Instrument;

pub use feed::{EventFeed, HeldWatches, InstrumentedMeta, LiveListeners};
pub use routes::sessions::{ConvertSession, ImportSession, ImportTarget};
pub use session_backend::SessionPlaneBackend;

/// SSE keep-alive default — `PEREGRIN_SSE_KEEPALIVE_MS`'s fallback,
/// duplicated here so `SessionState` doesn't take an `Env`.
const DEFAULT_SSE_KEEPALIVE_MS: u64 = 15_000;
/// Held-session watch re-probe default — `PEREGRIN_HELD_WATCH_MS`'s
/// fallback.
const DEFAULT_HELD_WATCH_MS: u64 = 5_000;

/// Shared state threaded through every session handler — the
/// `SessionPlane` port plus the side-channels the routes own. Clone is
/// cheap (every field is an `Arc`/handle).
#[derive(Clone)]
pub struct SessionState {
    /// The session control port — `LocalPlane` in-process or the
    /// `sepia serve` wire plane.
    pub plane: Arc<dyn SessionPlane>,
    /// The instrumented meta overlay — session metas only since P-006
    /// phase 2 (projects/config moved to [`Self::kv`]); `None` → meta
    /// routes answer 501.
    pub meta: Option<InstrumentedMeta>,
    /// Node-local KV store (P-006) — the `project`/`config`/`push`
    /// namespaces behind the `/api/projects*` + `/api/config*` compat
    /// routes and, shared at composition, the substrate's `/api/kv*`.
    /// `None` → those surfaces answer 501.
    pub kv: Option<Arc<dyn hypha_kv::KvStore>>,
    /// Process-wide node event feed — the same broadcast the
    /// substrate's `/api/events` serves at composition.
    pub feed: EventFeed,
    /// Identity reported in `RunSpan.node` provenance — `PATCH
    /// /api/node` renames ride the substrate state's own copy; this
    /// one follows the builders.
    pub node: Arc<Mutex<NodeIdentity>>,
    /// Pairing backend — credentials `/api/pair` mints are visible to
    /// this router's auth gate (shared instance at composition).
    pub pairing: Option<hypha_http::pair::Pairing>,
    /// `PEREGRIN_TOKEN` — `None`/empty means bearer auth is off entirely.
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
    /// Web Push service — subscriptions persist in the KV `push`
    /// namespace ([`push::KvPushBackend`]); shared with the substrate's
    /// `/api/push/*` at composition. `None` → 501s.
    pub push: Option<Arc<hypha_push::PushStore>>,
}

impl SessionState {
    /// Minimal state — tokenless, no meta store, ephemeral node id.
    pub fn new(plane: Arc<dyn SessionPlane>) -> Self {
        Self {
            plane,
            meta: None,
            kv: None,
            feed: EventFeed::new(),
            node: Arc::new(Mutex::new(NodeIdentity {
                id: format!("node_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
                name: hostname_or_unknown(),
            })),
            pairing: None,
            token: None,
            origins: vec![
                "http://localhost:13000".to_string(),
                "http://127.0.0.1:13000".to_string(),
            ],
            keep_alive: Duration::from_millis(DEFAULT_SSE_KEEPALIVE_MS),
            held: HeldWatches::new(Duration::from_millis(DEFAULT_HELD_WATCH_MS)),
            live: LiveListeners::default(),
            import_session: None,
            convert: None,
            push: None,
        }
    }

    #[must_use]
    pub fn with_meta(mut self, meta: MetaStore) -> Self {
        self.meta = Some(InstrumentedMeta::new(meta, self.feed.clone()));
        self
    }

    /// Install the node-local KV store — `None` leaves the
    /// `/api/projects*` + `/api/config*` compat surfaces answering 501.
    #[must_use]
    pub fn with_kv(mut self, kv: Option<Arc<dyn hypha_kv::KvStore>>) -> Self {
        self.kv = kv;
        self
    }

    #[must_use]
    pub fn with_pairing(mut self, pairing: hypha_http::pair::Pairing) -> Self {
        self.pairing = Some(pairing);
        self
    }

    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
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

    #[must_use]
    pub fn with_convert(mut self, f: ConvertSession) -> Self {
        self.convert = Some(f);
        self
    }

    /// Override the push service (tests wire a stub store).
    #[must_use]
    pub fn with_push(mut self, push: Option<Arc<hypha_push::PushStore>>) -> Self {
        self.push = push;
        self
    }

    #[must_use]
    pub fn with_node(mut self, node: NodeIdentity) -> Self {
        self.node = Arc::new(Mutex::new(node));
        self
    }

    /// Snapshot of the node identity — one lock, one clone.
    pub fn node_identity(&self) -> NodeIdentity {
        self.node
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The node id `RunSpan.node` reports — the identity's `id` field.
    pub fn node_id(&self) -> String {
        self.node_identity().id
    }
}

/// The `capabilities` list `GET /api/node` reports for the session
/// surface — `sessions`/`projects`/`push`/`events`/`export`, plus
/// `pairing`/`import` when those backends are wired. The substrate
/// honors it via `HyphaState::node_capabilities`.
#[must_use]
pub fn node_capabilities(state: &SessionState) -> Vec<String> {
    let mut capabilities = vec![
        "sessions".to_string(),
        "projects".to_string(),
        "push".to_string(),
        "events".to_string(),
        "export".to_string(),
    ];
    if state.pairing.is_some() {
        capabilities.push("pairing".into());
    }
    // The store-write seam (`POST /api/sessions/import`) is only wired
    // when a convertible store exists — the hub's transfer pre-flight
    // reads this flag instead of eating a 501.
    if state.convert.is_some() {
        capabilities.push("import".into());
    }
    capabilities
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
pub async fn cors_layer(
    State(state): State<SessionState>,
    req: Request<Body>,
    next: Next,
) -> Response {
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

/// OTEL server span — the outermost layer. Extracts the W3C remote
/// parent (`traceparent`/`tracestate`) so proxied hub→node calls join
/// the hub's trace; requests without context get a local-root
/// `http_request` span. Inert (plain span) when export is off.
pub async fn trace_layer(req: Request<Body>, next: Next) -> Response {
    let span = lampy_telemetry_otel::server_span(req.method(), req.uri().path(), req.headers());
    let for_status = span.clone();
    async move {
        let res = next.run(req).await;
        lampy_telemetry_otel::record_response(&for_status, res.status().as_u16());
        res
    }
    .instrument(span)
    .await
}

/// One line per request — `/api/health` excluded, like the TS logger.
pub async fn log_layer(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let started = Instant::now();
    let res = next.run(req).await;
    if path != "/api/health" {
        // Hoisted — `tracing` fields evaluate only when a subscriber
        // is attached, which coverage runs don't guarantee.
        let status = res.status().as_u16();
        let elapsed_ms = started.elapsed().as_millis();
        tracing::info!("{method} {path} {status} {elapsed_ms}ms");
    }
    res
}

/// The session-surface router — `/api/sessions*`, agents, projects,
/// config — with the auth gate, request log, CORS and trace layers,
/// fallback-free for merging (the host's merged router supplies the
/// 404). Every route is `any()` + an internal method guard so
/// wrong-method requests fall through to the 404 (rather than axum's
/// bare 405). The generic substrate routes are `hypha-http`'s —
/// hosts merge the two under one listener.
pub fn app_routes(state: &SessionState) -> Router<SessionState> {
    // The held-session watch ticker — re-probes lock state for sessions
    // last seen read-only while the feed has listeners.
    state
        .held
        .spawn_ticker(Arc::clone(&state.plane), state.feed.clone());

    Router::new()
        .route("/api/agents", any(routes::agents::agents))
        // `/api/client/keypair` is hypha-http's (`/api/kv*` phase 1).
        .route("/api/config", any(routes::config::get))
        .route("/api/config/{key}", any(routes::config::set))
        .route("/api/projects", any(routes::projects::collection))
        .route("/api/projects/{id}", any(routes::projects::item))
        .route("/api/sessions", any(routes::sessions::collection))
        .route(
            "/api/sessions/import",
            any(routes::sessions::import_session),
        )
        .route("/api/sessions/run", any(routes::sessions::run))
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
        // Innermost → outermost: auth gate, request log, CORS, trace.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_gate,
        ))
        .layer(middleware::from_fn(log_layer))
        .layer(middleware::from_fn_with_state(state.clone(), cors_layer))
        .layer(middleware::from_fn(trace_layer))
}

/// The standalone session surface — [`app_routes`] plus the 404
/// fallback and state binding, for hosts that mount the session
/// surface without the substrate (tests, tooling).
pub fn app(state: SessionState) -> Router {
    app_routes(&state)
        .fallback(async || routes::not_found())
        .with_state(state)
}

/// Re-export for hosts composing the substrate — the entity-plane
/// port and pairing types stay `hypha-http`'s.
pub use hypha_http::entity_plane;
pub use hypha_http::entity_plane::{EntityContext, EntityPlane, KindRegistry};
pub use hypha_http::pair;
pub use hypha_http::{Pairing, PairingStore, Quota, RateLimiter};
