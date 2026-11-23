//! Server-persisted app/UI config — `routes/config.ts` ported onto the
//! KV `config` namespace (P-006 phase 2). Keys in [`crate::kv::INTERNAL_CONFIG`]
//! (secrets/blobs — they live in the internal `push` namespace now)
//! are never exposed and can't be written through this compat route.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};

use crate::SessionState;
use crate::kv::{INTERNAL_CONFIG, NS_CONFIG};
use crate::routes::{SMALL_JSON_BODY, json_response, not_found, read_json_body};

fn kv_missing() -> Response {
    json_response(
        json!({ "error": "KV store is not configured on this server" }),
        StatusCode::NOT_IMPLEMENTED,
    )
}

/// `GET /api/config` — the public config view (internal keys filtered;
/// they shouldn't be in the public namespace at all, the filter is the
/// belt to migration's suspenders).
pub async fn get(
    State(state): State<SessionState>,
    method: Method,
    _req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let Some(store) = &state.kv else {
        return kv_missing();
    };
    let config: serde_json::Map<String, Value> = store
        .list(NS_CONFIG)
        .into_iter()
        .filter(|(k, _)| !INTERNAL_CONFIG.contains(&k.as_str()))
        .collect();
    json_response(json!({ "config": config }), StatusCode::OK)
}

/// `PATCH /api/config/{key}` — `{value}` stored verbatim in the KV
/// `config` namespace. Silent on the feed, like the meta-backed
/// version — config writes never emitted events.
pub async fn set(
    State(state): State<SessionState>,
    method: Method,
    Path(key): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::PATCH {
        return not_found();
    }
    let Some(store) = &state.kv else {
        return kv_missing();
    };
    let body = match read_json_body(req, SMALL_JSON_BODY).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    if INTERNAL_CONFIG.contains(&key.as_str()) {
        return json_response(
            json!({ "error": "Config key is internal" }),
            StatusCode::BAD_REQUEST,
        );
    }
    let value = body
        .as_ref()
        .and_then(|b| b.as_object())
        .and_then(|o| o.get("value").cloned())
        .unwrap_or(Value::Null);
    store.set(NS_CONFIG, &key, value.clone());
    json_response(json!({ "key": key, "value": value }), StatusCode::OK)
}
