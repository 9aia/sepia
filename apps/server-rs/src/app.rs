//! HTTP surface — mirrors `apps/server/src/app.ts`: route registration,
//! bearer auth (query-token only on the two SSE GETs), CORS, access log.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, patch, post};
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::env::ServerEnv;
use crate::error::{Error, Result};
use crate::node::{self, NodeIdentity, PROTOCOL_VERSION};

#[derive(Clone)]
pub struct AppState {
    pub env: Arc<ServerEnv>,
    /// Node identity — name is mutable via PATCH /api/node.
    pub node: Arc<Mutex<NodeIdentity>>,
}

/// `?access_token` is honored only on the SSE GETs — URLs leak into logs and
/// history, so every other route requires the Authorization header.
fn is_sse_path(path: &str) -> bool {
    path == "/api/events" || (path.starts_with("/api/sessions/") && path.ends_with("/stream"))
}

fn authorized(headers: &HeaderMap, uri: &Uri, token: Option<&str>) -> bool {
    let Some(expected) = token else {
        return true;
    };
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        && value == format!("Bearer {expected}")
    {
        return true;
    }
    if is_sse_path(uri.path())
        && let Some(query) = uri.query()
    {
        for pair in query.split('&') {
            if let Some(v) = pair.strip_prefix("access_token=")
                && v == expected
            {
                return true;
            }
        }
    }
    false
}

async fn auth_middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    // Pairing is the credential bootstrap — the one-time code authorizes it;
    // health stays open for liveness probes.
    let open = path == "/api/health" || path == "/api/pair";
    if path.starts_with("/api/")
        && !open
        && !authorized(request.headers(), request.uri(), state.env.token.as_deref())
    {
        return Error::Unauthorized("Unauthorized".to_string()).into_response();
    }
    next.run(request).await
}

/// Access log — `GET /api/health` is excluded (probe noise), same as the Bun server.
async fn log_middleware(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if path != "/api/health" {
        tracing::info!(
            "{method} {path} {} {}ms",
            response.status().as_u16(),
            started.elapsed().as_millis()
        );
    }
    response
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn get_node(State(state): State<AppState>) -> Json<serde_json::Value> {
    let node = state.node.lock().unwrap_or_else(PoisonError::into_inner);
    Json(json!({
        "id": node.id,
        "name": node.name,
        "version": node.version,
        "protocol": PROTOCOL_VERSION,
        "agents": [],
        "capabilities": ["sessions", "projects", "push", "events", "export", "transfer"],
    }))
}

#[derive(Debug, Deserialize)]
struct RenameBody {
    name: Option<String>,
}

async fn patch_node(
    State(state): State<AppState>,
    Json(body): Json<RenameBody>,
) -> Result<Json<serde_json::Value>> {
    let name = body.name.unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("name is required".to_string()));
    }
    {
        let mut node = state.node.lock().unwrap_or_else(PoisonError::into_inner);
        node.name = name.to_string();
    }
    node::rename_node(&state.env.node_path, name)
        .map_err(|e| Error::Internal(anyhow::anyhow!("persist node name: {e}")))?;
    Ok(get_node(State(state)).await)
}

/// `ring` keypair mint — Ed25519 first, ECDSA P-256 fallback. The client's
/// secretKey is an opaque blob (PKCS8 DER here vs JWK `d` on the Bun server)
/// — the web client stores it verbatim and the wire contract holds.
fn mint_keypair() -> Result<Json<serde_json::Value>> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair};

    let rng = SystemRandom::new();
    if let Ok(pkcs8) = Ed25519KeyPair::generate_pkcs8(&rng) {
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|e| Error::Internal(anyhow::anyhow!("ed25519 reparse: {e}")))?;
        return Ok(Json(json!({
            "algorithm": "Ed25519",
            "publicKey": URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
            "secretKey": URL_SAFE_NO_PAD.encode(pkcs8.as_ref()),
        })));
    }
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
        .map_err(|e| Error::Internal(anyhow::anyhow!("ecdsa generate: {e}")))?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
        .map_err(|e| Error::Internal(anyhow::anyhow!("ecdsa reparse: {e}")))?;
    Ok(Json(json!({
        "algorithm": "ECDSA-P-256",
        "publicKey": URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
        "secretKey": URL_SAFE_NO_PAD.encode(pkcs8.as_ref()),
    })))
}

async fn client_keypair() -> Result<Json<serde_json::Value>> {
    mint_keypair()
}

pub fn build_app(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/node", get(get_node))
        .route("/api/node", patch(patch_node))
        .route("/api/client/keypair", post(client_keypair))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(
            state
                .env
                .origins
                .iter()
                .filter_map(|o| o.parse::<HeaderValue>().ok()),
        ))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .merge(api)
        .layer(cors)
        .layer(middleware::from_fn(log_middleware))
        .with_state(state)
}
