//! `GET /api/agents` — the agent runtime inventory (`plane.listAgents`).

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::json;

use crate::AppState;
use crate::routes::{AgentWire, json_response, not_found};

pub async fn agents(
    State(state): State<AppState>,
    method: Method,
    _req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let agents: Vec<AgentWire> = state
        .plane
        .list_agents()
        .await
        .into_iter()
        .map(|a| AgentWire {
            id: a.id,
            label: a.label,
            capabilities: a.capabilities.as_ref().map(Into::into),
        })
        .collect();
    json_response(json!({ "agents": agents }), StatusCode::OK)
}
