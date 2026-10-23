//! `sepia serve` — the local session plane as a service: ndjson
//! JSON-RPC over stdio, the same framing `sepia-driver-*` binaries
//! speak (`sepia_driver::rpc`). This is the interface orchestrators
//! consume — peregrin spawns this process exactly like a driver.
//!
//! Method surface (params are camelCase JSON):
//!
//! | method | plane call |
//! | --- | --- |
//! | `manifest` | service manifest + method list |
//! | `agents.list` | `ControlPlane::list_agents` |
//! | `sessions.list` | `list_sessions(with_locks?)` |
//! | `sessions.get` / `sessions.summary` | `get_session` / `get_summary` |
//! | `sessions.history` | `get_history` |
//! | `sessions.attach` / `sessions.detach` | `attach` / `detach` |
//! | `sessions.create` | `create_session` |
//! | `sessions.prompt` / `sessions.cancel` | `prompt` / `cancel` |
//! | `sessions.permission` | `respond_to_permission` |
//! | `sessions.delete` | `delete_session` |
//! | `sessions.patch_meta` | `patch_meta` — the overlay (title/pin/archive/projects/model) |
//! | `projects.list` / `projects.create` | `list_projects` / `create_project` |
//! | `projects.rename` / `projects.delete` | `rename_project` / `delete_project` |
//! | `sessions.restore` / `sessions.rewind` | `restore` / `rewind` |
//! | `sessions.subscribe` | `subscribe` → `session.event` notifications |
//! | `shutdown` | close the plane, exit 0 |

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use sepia_driver::rpc::{Inbound, Request, RpcError, decode_line, encode_line};
use sepia_session::{
    HistoryOptions, Local, Paths, RestoreRequest, RewindRequest, base_options, build, sepia_home,
};
use serde_json::{Value, json};

use crate::CliError;

/// Wire protocol version — bumped on any breaking change to the method
/// surface; `manifest` reports it so spawners can negotiate.
const PROTOCOL: u64 = 1;

/// Outbound line sink shared between the dispatch loop and subscription
/// forwarders — one lock per line keeps writes atomic.
type Out = Arc<Mutex<std::io::BufWriter<std::io::Stdout>>>;

fn write_line(out: &Out, message: &impl serde::Serialize) {
    let Ok(line) = encode_line(message) else {
        return;
    };
    if let Ok(mut w) = out.lock() {
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
}

fn control_err(e: &sepia_session::ControlError) -> RpcError {
    let mut err = RpcError::internal(e.message.clone());
    err.data = Some(json!({ "code": e.code.as_str(), "cause": e.cause }));
    err
}

fn id_of(params: &Value) -> Result<&str, RpcError> {
    params
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing session id"))
}

fn agent_of(params: &Value) -> Option<String> {
    params
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn manifest(identity: &sepia_session::NodeIdentity) -> Value {
    json!({
        "name": "sepia",
        "version": crate::SEPIA_VERSION,
        "protocol": PROTOCOL,
        "node": { "id": identity.id, "name": identity.name },
        "methods": [
            "manifest", "agents.list",
            "sessions.list", "sessions.get", "sessions.summary",
            "sessions.history", "sessions.attach", "sessions.detach",
            "sessions.create", "sessions.prompt", "sessions.cancel",
            "sessions.permission", "sessions.delete", "sessions.patch_meta",
            "sessions.restore", "sessions.rewind", "sessions.subscribe",
            "projects.list", "projects.create", "projects.rename",
            "projects.delete", "shutdown",
        ],
    })
}

async fn dispatch(local: &Local, req: &Request, out: &Out) -> Result<Value, RpcError> {
    let plane = &local.plane;
    let agent = agent_of(&req.params);
    match req.method.as_str() {
        "manifest" => Ok(manifest(&local.identity)),
        "agents.list" => {
            let agents: Vec<Value> = plane
                .list_agents()
                .await
                .iter()
                .map(|a| {
                    json!({
                        "id": a.id,
                        "label": a.label,
                        "capabilities": a.capabilities,
                    })
                })
                .collect();
            Ok(json!(agents))
        }
        "sessions.list" => {
            let with_locks = req
                .params
                .get("withLocks")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            plane
                .list_sessions(with_locks)
                .await
                .map_err(|e| control_err(&e))
                .and_then(|v| {
                    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
                })
        }
        "sessions.get" | "sessions.summary" => {
            let id = id_of(&req.params)?;
            let session = if req.method == "sessions.get" {
                plane.get_session(id, agent.as_deref()).await
            } else {
                plane.get_summary(id, agent.as_deref()).await
            }
            .map_err(|e| control_err(&e))?;
            sepia_core::wire::session_to_json(&session)
                .map_err(|e| RpcError::internal(e.to_string()))
        }
        "sessions.history" => {
            let id = id_of(&req.params)?;
            let options = HistoryOptions {
                limit: req
                    .params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
                before: req.params.get("before").and_then(Value::as_i64),
                agent_id: agent,
            };
            plane
                .get_history(id, &options)
                .await
                .map_err(|e| control_err(&e))
                .and_then(|v| {
                    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
                })
        }
        "sessions.attach" => {
            let id = id_of(&req.params)?;
            let takeover = req
                .params
                .get("takeover")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let model = req
                .params
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string);
            let fallbacks = req
                .params
                .get("fallbacks")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                });
            let result = plane
                .attach(id, takeover, model, fallbacks, agent)
                .await
                .map_err(|e| control_err(&e))?;
            Ok(json!({
                "attached": result.attached,
                "readOnly": result.read_only,
                "agentId": result.agent_id,
                "capabilities": result.capabilities,
            }))
        }
        "sessions.detach" => {
            plane.detach(id_of(&req.params)?).await;
            Ok(Value::Null)
        }
        "sessions.create" => {
            let cwd = req
                .params
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing cwd"))?;
            let title = req
                .params
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            let model = req
                .params
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string);
            let fallbacks = req
                .params
                .get("fallbacks")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                });
            let result = plane
                .create_session(cwd, agent, title, model, fallbacks)
                .await
                .map_err(|e| control_err(&e))?;
            Ok(json!({
                "id": result.id,
                "agentId": result.agent_id,
                "capabilities": result.capabilities,
            }))
        }
        "sessions.prompt" => {
            let id = id_of(&req.params)?;
            let parts: Vec<sepia_acp::types::PromptPart> = serde_json::from_value(
                req.params
                    .get("parts")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            plane
                .prompt(id, &parts, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.cancel" => {
            plane
                .cancel(id_of(&req.params)?, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.permission" => {
            let id = id_of(&req.params)?;
            let request_id = req
                .params
                .get("request")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing request id"))?;
            let option = req.params.get("option").and_then(Value::as_str);
            plane
                .respond_to_permission(id, request_id, option, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.delete" => {
            plane
                .delete_session(id_of(&req.params)?, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.patch_meta" => {
            let id = id_of(&req.params)?;
            let raw = req.params.get("patch").cloned().unwrap_or_default();
            let mut patch: sepia_session::SessionMeta = serde_json::from_value(raw.clone())
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            // `Option<Option<String>>` flattens `"model": null` into
            // "keep" — restore the documented explicit clear.
            if raw.get("model").is_some() {
                patch.model = match raw["model"].as_str() {
                    Some(s) => Some(Some(s.to_string())),
                    None => Some(None),
                };
            }
            plane
                .patch_meta(id, &patch)
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "projects.list" => serde_json::to_value(plane.list_projects())
            .map_err(|e| RpcError::internal(e.to_string())),
        "projects.create" => {
            let name = req
                .params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing project name"))?;
            serde_json::to_value(plane.create_project(name).map_err(|e| control_err(&e))?)
                .map_err(|e| RpcError::internal(e.to_string()))
        }
        "projects.rename" => {
            let id = req
                .params
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing project id"))?;
            let name = req
                .params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing project name"))?;
            plane
                .rename_project(id, name)
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "projects.delete" => {
            let id = req
                .params
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing project id"))?;
            plane.delete_project(id).map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.restore" | "sessions.rewind" => {
            let id = id_of(&req.params)?;
            let result = if req.method == "sessions.restore" {
                let request: RestoreRequest =
                    serde_json::from_value(req.params.get("request").cloned().unwrap_or_default())
                        .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                serde_json::to_value(
                    plane
                        .restore(id, &request, agent.as_deref())
                        .await
                        .map_err(|e| control_err(&e))?,
                )
            } else {
                let request: RewindRequest =
                    serde_json::from_value(req.params.get("request").cloned().unwrap_or_default())
                        .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                serde_json::to_value(
                    plane
                        .rewind(id, &request, agent.as_deref())
                        .await
                        .map_err(|e| control_err(&e))?,
                )
            };
            result.map_err(|e| RpcError::internal(e.to_string()))
        }
        "sessions.subscribe" => {
            let id = id_of(&req.params)?.to_string();
            let mut rx = plane
                .subscribe(&id, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            let out = Arc::clone(out);
            let session_id = id.clone();
            tokio::spawn(async move {
                while let Ok(events) = rx.recv().await {
                    write_line(
                        &out,
                        &sepia_driver::rpc::Notification::new(
                            "session.event",
                            json!({ "sessionId": session_id, "events": events }),
                        ),
                    );
                }
            });
            Ok(json!({ "subscribed": true, "sessionId": id }))
        }
        other => Err(RpcError::new(-32601, format!("unknown method: {other}"))),
    }
}

/// Build the local plane and serve JSON-RPC on stdio until `shutdown`
/// or EOF. Multi-thread runtime: subscriptions forward concurrently.
///
/// # Errors
/// `CliError` on plane build or stdin read failure.
pub fn serve_stdio() -> Result<(), CliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError(format!("failed to start runtime: {e}")))?;
    runtime.block_on(async {
        let paths = Paths::new(sepia_home());
        let local = build(&paths, base_options())
            .await
            .map_err(|e| CliError(format!("failed to build session plane: {e}")))?;
        let out: Out = Arc::new(Mutex::new(std::io::BufWriter::new(std::io::stdout())));
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| CliError(format!("stdin read failed: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            match decode_line(&line) {
                Ok(Inbound::Request(req)) => {
                    if req.method == "shutdown" {
                        write_line(&out, &sepia_driver::rpc::Response::ok(req.id, Value::Null));
                        break;
                    }
                    let response = match dispatch(&local, &req, &out).await {
                        Ok(result) => sepia_driver::rpc::Response::ok(req.id, result),
                        Err(error) => sepia_driver::rpc::Response::err(req.id, error),
                    };
                    write_line(&out, &response);
                }
                Ok(Inbound::Notification(_) | Inbound::Response(_)) => {}
                Err((id, error)) => {
                    let response = sepia_driver::rpc::Response::err(id.unwrap_or(0), error);
                    write_line(&out, &response);
                }
            }
        }
        local.plane.close_all().await;
        Ok(())
    })
}
