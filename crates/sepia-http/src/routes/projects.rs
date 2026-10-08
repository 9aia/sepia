//! Project CRUD — `routes/projects.ts` ported: named groups over session
//! meta (`meta.projectIds`). All routes 501 without a meta store.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::json;

use crate::AppState;
use crate::routes::{json_response, not_found, ok, read_json_body};

fn meta_missing() -> Response {
    json_response(
        json!({ "error": "Meta is not configured on this server" }),
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
    State(state): State<AppState>,
    method: Method,
    req: Request<Body>,
) -> Response {
    match method {
        Method::GET => {
            let Some(meta) = &state.meta else {
                return meta_missing();
            };
            json_response(json!({ "projects": meta.list_projects() }), StatusCode::OK)
        }
        Method::POST => {
            let Some(meta) = &state.meta else {
                return meta_missing();
            };
            let body = match read_json_body(req).await {
                Ok(b) => b,
                Err(res) => return *res,
            };
            let name = match name_param(body.as_ref()) {
                Ok(n) => n,
                Err(res) => return *res,
            };
            json_response(
                json!({ "project": meta.create_project(&name) }),
                StatusCode::CREATED,
            )
        }
        _ => not_found(),
    }
}

/// `PATCH|DELETE /api/projects/{id}`.
pub async fn item(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    match method {
        Method::PATCH => {
            let Some(meta) = &state.meta else {
                return meta_missing();
            };
            let body = match read_json_body(req).await {
                Ok(b) => b,
                Err(res) => return *res,
            };
            let name = match name_param(body.as_ref()) {
                Ok(n) => n,
                Err(res) => return *res,
            };
            if !meta.rename_project(&id, &name) {
                return json_response(json!({ "error": "Unknown project" }), StatusCode::NOT_FOUND);
            }
            ok()
        }
        Method::DELETE => {
            let Some(meta) = &state.meta else {
                return meta_missing();
            };
            meta.delete_project(&id);
            ok()
        }
        _ => not_found(),
    }
}
