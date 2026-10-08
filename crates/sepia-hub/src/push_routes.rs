//! Hub-owned push subscriptions — one subscription store for every
//! node, so a browser subscribes once and gets notified by all of
//! them. `runFinished`/`permissionRequested` feed markers (emitted by
//! each node's live listener) drive the fan-out.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use sepia_push::{PushPrefs, PushSubscription, SubscriptionKeys};

use crate::HubState;

fn not_configured() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "error": "Push not configured" })),
    )
        .into_response()
}

/// `GET /api/push/vapid` — the hub's public key (same for every node).
pub async fn vapid(State(state): State<HubState>) -> Response {
    match &state.push {
        Some(push) => Json(json!({ "publicKey": push.public_key() })).into_response(),
        None => not_configured(),
    }
}

/// `POST`/`DELETE /api/push/subscribe`.
pub async fn subscribe(
    State(state): State<HubState>,
    method: axum::http::Method,
    body: String,
) -> Response {
    let Some(push) = &state.push else {
        return not_configured();
    };
    let body: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid JSON body"})))
                .into_response();
        }
    };
    if method == axum::http::Method::POST {
        let Some(obj) = body.as_object() else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid push subscription"})))
                .into_response();
        };
        let (Some(endpoint), Some(keys)) = (
            obj.get("endpoint").and_then(Value::as_str),
            obj.get("keys").and_then(Value::as_object),
        ) else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid push subscription"})))
                .into_response();
        };
        let (Some(auth), Some(p256dh)) = (
            keys.get("auth").and_then(Value::as_str),
            keys.get("p256dh").and_then(Value::as_str),
        ) else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid push subscription"})))
                .into_response();
        };
        let prefs = obj
            .get("prefs")
            .and_then(Value::as_object)
            .map(|p| PushPrefs {
                done: p.get("done").and_then(Value::as_bool) != Some(false),
                permission: p.get("permission").and_then(Value::as_bool) != Some(false),
            })
            .unwrap_or_default();
        push.upsert(PushSubscription {
            endpoint: endpoint.to_string(),
            keys: SubscriptionKeys {
                auth: auth.to_string(),
                p256dh: p256dh.to_string(),
            },
            prefs,
        });
    } else if let Some(endpoint) = body.get("endpoint").and_then(Value::as_str) {
        push.remove(endpoint);
    }
    Json(json!({ "ok": true })).into_response()
}
