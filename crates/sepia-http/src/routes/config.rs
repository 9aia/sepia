//! Server-persisted app/UI config — `routes/config.ts` ported. Keys in
//! `INTERNAL_CONFIG` (secrets/blobs) are never exposed and can't be
//! written.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};

use crate::AppState;
use crate::routes::{json_response, not_found, read_json_body};

const INTERNAL_CONFIG: [&str; 2] = ["vapid", "pushSubscriptions"];

fn meta_missing() -> Response {
    json_response(
        json!({ "error": "Meta is not configured on this server" }),
        StatusCode::NOT_IMPLEMENTED,
    )
}

/// `GET /api/config` — the public config view (internal keys filtered).
pub async fn get(State(state): State<AppState>, method: Method, _req: Request<Body>) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let Some(meta) = &state.meta else {
        return meta_missing();
    };
    let config: serde_json::Map<String, Value> = meta
        .config()
        .into_iter()
        .filter(|(k, _)| !INTERNAL_CONFIG.contains(&k.as_str()))
        .collect();
    json_response(json!({ "config": config }), StatusCode::OK)
}

/// `PATCH /api/config/{key}` — `{value}` stored verbatim.
pub async fn set(
    State(state): State<AppState>,
    method: Method,
    Path(key): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::PATCH {
        return not_found();
    }
    let Some(meta) = &state.meta else {
        return meta_missing();
    };
    let body = match read_json_body(req).await {
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
    meta.set_config(&key, value.clone());
    json_response(json!({ "key": key, "value": value }), StatusCode::OK)
}
