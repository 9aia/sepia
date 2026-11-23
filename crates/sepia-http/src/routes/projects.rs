//! Project CRUD — `routes/projects.ts` ported onto the KV `project`
//! namespace (P-006 phase 2): named groups whose ids ride session
//! meta's `projectIds`. Writes emit the same `project` feed events
//! `InstrumentedMeta` did; deletes cascade-strip the id from every
//! session meta (the `MetaStore::delete_project` half that stayed
//! behind in sepia-meta). All routes 501 without a KV store.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::json;

use crate::SessionState;
use crate::kv::{self, NS_PROJECT};
use crate::routes::{SMALL_JSON_BODY, json_response, not_found, ok, read_json_body};

fn kv_missing() -> Response {
    json_response(
        json!({ "error": "KV store is not configured on this server" }),
        StatusCode::NOT_IMPLEMENTED,
    )
}

fn name_param(body: Option<&serde_json::Value>) -> Result<String, Box<Response>> {
    let name = body
        .and_then(|b| b.as_object())
        .and_then(|o| o.get("name"))
        .and_then(|v| v.as_str());
    match name {
        Some(n) if !n.trim().is_empty() && n.len() <= 100 => Ok(n.trim().to_string()),
        _ => Err(Box::new(json_response(
            json!({ "error": "name must be a non-empty string (max 100)" }),
            StatusCode::BAD_REQUEST,
        ))),
    }
}

/// `GET|POST /api/projects`.
pub async fn collection(
    State(state): State<SessionState>,
    method: Method,
    req: Request<Body>,
) -> Response {
    match method {
        Method::GET => {
            let Some(store) = &state.kv else {
                return kv_missing();
            };
            json_response(
                json!({ "projects": kv::list_projects(store.as_ref()) }),
                StatusCode::OK,
            )
        }
        Method::POST => {
            let Some(store) = &state.kv else {
                return kv_missing();
            };
            let body = match read_json_body(req, SMALL_JSON_BODY).await {
                Ok(b) => b,
                Err(res) => return *res,
            };
            let name = match name_param(body.as_ref()) {
                Ok(n) => n,
                Err(res) => return *res,
            };
            let id = kv::new_project_id();
            store.set(NS_PROJECT, &id, json!({ "name": name }));
            state.feed.emit(
                "project",
                kv::project_set_payload(&id, &json!({ "name": name })),
            );
            json_response(
                json!({ "project": { "id": id, "name": name } }),
                StatusCode::CREATED,
            )
        }
        _ => not_found(),
    }
}

/// `PATCH|DELETE /api/projects/{id}`.
pub async fn item(
    State(state): State<SessionState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    match method {
        Method::PATCH => {
            let Some(store) = &state.kv else {
                return kv_missing();
            };
            let body = match read_json_body(req, SMALL_JSON_BODY).await {
                Ok(b) => b,
                Err(res) => return *res,
            };
            let name = match name_param(body.as_ref()) {
                Ok(n) => n,
                Err(res) => return *res,
            };
            // `rename_project`'s existence check — a missing key is a
            // 404, a present one takes the new name (sibling fields
            // preserved by `ensure_project`).
            if store.get(NS_PROJECT, &id).is_none() {
                return json_response(json!({ "error": "Unknown project" }), StatusCode::NOT_FOUND);
            }
            let _ = kv::ensure_project(store.as_ref(), &id, &name);
            state.feed.emit(
                "project",
                kv::project_set_payload(&id, &json!({ "name": name })),
            );
            ok()
        }
        Method::DELETE => {
            let Some(store) = &state.kv else {
                return kv_missing();
            };
            if store.delete(NS_PROJECT, &id) {
                // The cascade `MetaStore::delete_project` used to own:
                // strip the id from every session meta's projectIds,
                // silently (no `meta` feed events, like before).
                if let Some(meta) = &state.meta {
                    kv::strip_project_from_sessions(meta.store(), &id);
                }
                state.feed.emit("project", kv::project_delete_payload(&id));
            }
            ok()
        }
        _ => not_found(),
    }
}
