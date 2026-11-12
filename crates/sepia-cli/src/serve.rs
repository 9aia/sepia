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
//! | `sessions.export` | `get_session` wrapped as `{session}` — the transfer bundle |
//! | `sessions.import` | `Local::import_session` — write IR into an agent's store |
//! | `sessions.convert` | `Local::convert_session` — node-local store→store copy |
//! | `sessions.history` | `get_history` |
//! | `sessions.attach` / `sessions.detach` | `attach` / `detach` |
//! | `sessions.create` | `create_session` |
//! | `sessions.prompt` / `sessions.cancel` | `prompt` / `cancel` |
//! | `sessions.prompt_wait` | `prompt_wait` — prompt + await turn end → `PromptWaitResult` |
//! | `sessions.permission` | `respond_to_permission` |
//! | `sessions.set_permission_policy` | `set_permission_policy` — auto-answer `session/request_permission` |
//! | `sessions.delete` | `delete_session` |
//! | `sessions.patch_meta` | `patch_meta` — the overlay (title/pin/archive/projectIds/model) |
//! | `config.list` | `agent_config_summaries` — per-agent config item counts |
//! | `config.get` / `config.import` | `agent_config` / `write_agent_config` — the config IR over `config.read`/`config.write` |
//! | `sessions.restore` / `sessions.rewind` | `restore` / `rewind` |
//! | `sessions.subscribe` | `subscribe` → `session.event` notifications; |
//! | | `keepAlive:false` → non-pinning `watch` |
//! | `events.subscribe` | `subscribe_events` → `node.event` notifications: |
//! | | lifecycle markers + every live session's events |
//! | `shutdown` | close the plane, exit 0 |

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sepia_driver::rpc::{Inbound, Request, RpcError, decode_line, encode_line};
use sepia_session::{
    ControlError, ControlErrorCode, HistoryOptions, ImportOverrides, Local, NodeEvent, Paths,
    PermissionPolicy, RestoreRequest, RewindRequest, base_options, build, sepia_home,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::CliError;

/// Wire protocol version — bumped on any breaking change to the method
/// surface; `manifest` reports it so spawners can negotiate.
/// v2: the `projects.*` meta-overlay verbs are gone — projects/config
/// moved to peregrin's node-local KV (P-006 phase 3).
const PROTOCOL: u64 = 2;

/// Outbound line sink shared between the dispatch loop and subscription
/// forwarders — one lock per line keeps writes atomic.
type Out = Arc<Mutex<std::io::BufWriter<std::io::Stdout>>>;

fn write_line(out: &Out, message: &impl serde::Serialize) {
    // Notifications are always encodable; a blank line keeps the
    // protocol shape on catastrophic failure.
    let line = encode_line(message).unwrap_or_default();
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

fn str_param(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The `permissions` param — a [`PermissionPolicy`]: `"ask"`,
/// `"allow_all"`, `"deny_all"`, or `{"rules": [{"kind": "execute",
/// "allow": false}, …]}` (unmatched requests fall back to `ask`).
/// Absent → `None` (the session keeps `ask`).
fn policy_param(params: &Value) -> Result<Option<PermissionPolicy>, RpcError> {
    params
        .get("permissions")
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| RpcError::invalid_params(format!("invalid permissions: {e}")))
        })
        .transpose()
}

fn manifest(local: &Local) -> Value {
    json!({
        "name": "sepia",
        "version": crate::SEPIA_VERSION,
        "protocol": PROTOCOL,
        "node": { "id": local.identity.id, "name": local.identity.name },
        // The plane's resolved lifecycle tuning — lets a spawner verify
        // the SEPIA_* env it set on this process took effect.
        "config": local.plane.config(),
        "methods": [
            "manifest", "agents.list",
            "sessions.list", "sessions.get", "sessions.summary",
            "sessions.export", "sessions.import", "sessions.convert",
            "sessions.history", "sessions.attach", "sessions.detach",
            "sessions.create", "sessions.prompt", "sessions.prompt_wait",
            "sessions.cancel",
            "sessions.permission", "sessions.set_mode", "sessions.delete", "sessions.patch_meta",
            "sessions.set_permission_policy",
            "sessions.restore", "sessions.rewind", "sessions.subscribe",
            "events.subscribe",
            "config.list", "config.get", "config.import",
            "shutdown",
        ],
    })
}

async fn dispatch(local: &Local, req: &Request, out: &Out) -> Result<Value, RpcError> {
    let plane = &local.plane;
    let agent = agent_of(&req.params);
    match req.method.as_str() {
        "manifest" => Ok(manifest(local)),
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
        "sessions.export" => {
            let id = id_of(&req.params)?;
            let session = plane
                .get_session(id, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            let session = sepia_core::wire::session_to_json(&session)
                .map_err(|e| RpcError::internal(e.to_string()))?;
            Ok(json!({ "session": session }))
        }
        "sessions.import" => {
            let agent = agent.ok_or_else(|| RpcError::invalid_params("missing agent"))?;
            let session = sepia_core::wire::session_from_json(
                req.params.get("session").unwrap_or(&Value::Null),
            )
            .map_err(|e| {
                control_err(&ControlError::caused(
                    ControlErrorCode::Invalid,
                    "session is not a session IR object",
                    e,
                ))
            })?;
            let overrides = ImportOverrides {
                id: str_param(&req.params, "id").filter(|id| !id.is_empty()),
                cwd: str_param(&req.params, "cwd"),
                title: str_param(&req.params, "title"),
                model: str_param(&req.params, "model"),
            };
            let id = local
                .import_session(&agent, &session, &overrides)
                .await
                .map_err(|e| control_err(&e))?;
            Ok(json!({ "id": id, "agentId": agent }))
        }
        "sessions.convert" => {
            let id = id_of(&req.params)?;
            let agent = agent.ok_or_else(|| RpcError::invalid_params("missing agent"))?;
            let from = str_param(&req.params, "from");
            let session_id = local
                .convert_session(id, &agent, from.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(json!({ "sessionId": session_id, "agentId": agent }))
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
            // `permissions` sets the auto-answer policy for
            // `session/request_permission` — applied only to a live
            // attach; a read-only (`attached: false`) result keeps `ask`.
            let policy = policy_param(&req.params)?;
            let result = plane
                .attach(id, takeover, model, fallbacks, agent.clone())
                .await
                .map_err(|e| control_err(&e))?;
            let applied = policy.clone().filter(|_| result.attached);
            if let Some(p) = &applied {
                plane
                    .set_permission_policy(id, p.clone(), agent.as_deref())
                    .await
                    .map_err(|e| control_err(&e))?;
            }
            Ok(json!({
                "attached": result.attached,
                "readOnly": result.read_only,
                "agentId": result.agent_id,
                "capabilities": result.capabilities,
                "permissions": applied.unwrap_or_default(),
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
            // `permissions` is the create-time auto-answer policy for
            // `session/request_permission` — safe to apply after create:
            // requests only arise during `session/prompt`, which needs
            // the id this call returns.
            let policy = policy_param(&req.params)?;
            let result = plane
                .create_session(cwd, agent.clone(), title, model, fallbacks)
                .await
                .map_err(|e| control_err(&e))?;
            if let Some(policy) = &policy {
                plane
                    .set_permission_policy(&result.id, policy.clone(), agent.as_deref())
                    .await
                    .map_err(|e| control_err(&e))?;
            }
            Ok(json!({
                "id": result.id,
                "agentId": result.agent_id,
                "capabilities": result.capabilities,
                "permissions": policy.unwrap_or_default(),
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
        "sessions.prompt_wait" => {
            let id = id_of(&req.params)?;
            let parts: Vec<sepia_acp::types::PromptPart> = serde_json::from_value(
                req.params
                    .get("parts")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            // `timeoutMs` bounds the wait; 0/absent waits until the
            // turn settles (bounded only by agent liveness). On expiry
            // the turn is cancelled — see `ControlPlane::prompt_wait`.
            let timeout_ms = req
                .params
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let timeout = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms));
            let result = plane
                .prompt_wait(id, &parts, agent.as_deref(), timeout)
                .await
                .map_err(|e| control_err(&e))?;
            serde_json::to_value(result).map_err(|e| RpcError::internal(e.to_string()))
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
        "sessions.set_permission_policy" => {
            let id = id_of(&req.params)?;
            let policy = policy_param(&req.params)?
                .ok_or_else(|| RpcError::invalid_params("missing permissions"))?;
            plane
                .set_permission_policy(id, policy, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.set_mode" => {
            let id = id_of(&req.params)?;
            let mode = req
                .params
                .get("mode")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("missing mode"))?;
            plane
                .set_mode(id, mode, agent.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            Ok(Value::Null)
        }
        "sessions.delete" => {
            let id = id_of(&req.params)?;
            match plane.delete_session(id, agent.as_deref()).await {
                Ok(()) => Ok(Value::Null),
                // Agents without a runtime (cursor) can't serve ACP
                // `session/delete` — fall back to the store-level delete
                // over the driver wire when the request names the agent.
                Err(e)
                    if e.code == sepia_session::ControlErrorCode::UnknownAgent
                        && agent.is_some() =>
                {
                    local
                        .delete_session_store(agent.as_deref().unwrap_or_default(), id)
                        .await
                        .map_err(|e| control_err(&e))?;
                    Ok(Value::Null)
                }
                Err(e) => Err(control_err(&e)),
            }
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
        "config.list" => {
            let dir = str_param(&req.params, "dir");
            serde_json::to_value(
                local
                    .agent_config_summaries(agent.as_deref(), dir.as_deref())
                    .await
                    .map_err(|e| control_err(&e))?,
            )
            .map_err(|e| RpcError::internal(e.to_string()))
        }
        "config.get" => {
            let agent = agent.ok_or_else(|| RpcError::invalid_params("missing agent"))?;
            let dir = str_param(&req.params, "dir");
            let config = local
                .agent_config(&agent, dir.as_deref())
                .await
                .map_err(|e| control_err(&e))?;
            serde_json::to_value(config).map_err(|e| RpcError::internal(e.to_string()))
        }
        "config.import" => {
            let agent = agent.ok_or_else(|| RpcError::invalid_params("missing agent"))?;
            let dir = str_param(&req.params, "dir");
            let config = sepia_core::agent_config::config_from_json(
                req.params.get("config").unwrap_or(&Value::Null),
            )
            .map_err(|e| {
                control_err(&ControlError::caused(
                    ControlErrorCode::Invalid,
                    "config is not a config IR object",
                    e,
                ))
            })?;
            let actions = local
                .write_agent_config(&agent, dir.as_deref(), &config)
                .await
                .map_err(|e| control_err(&e))?;
            Ok(json!({ "actions": actions }))
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
            // `keepAlive` (default true) marks the receiver as a listener
            // for idle tracking — it holds the agent subprocess open.
            // `false` is the non-pinning watch: same `session.event`
            // stream, but the session still idle-detaches on TTL —
            // the shape background consumers (orchestrator busy-tracking,
            // log mirrors) should use so they can't leak agent processes.
            let keep_alive = req
                .params
                .get("keepAlive")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut rx = if keep_alive {
                plane.subscribe(&id, agent.as_deref()).await
            } else {
                plane.watch(&id, agent.as_deref()).await
            }
            .map_err(|e| control_err(&e))?;
            let out = Arc::clone(out);
            let session_id = id.clone();
            // Note: repeat subscribes on one session each spawn a
            // forwarder — the stream multiplexes but notifications
            // duplicate. `events.subscribe` (one feed for the whole
            // node) is the manageable shape for that.
            tokio::spawn(async move {
                loop {
                    let params = match rx.recv().await {
                        Ok(events) => json!({ "sessionId": session_id, "events": events }),
                        // Honest loss: report the dropped-batch count
                        // instead of silently ending the stream.
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            json!({ "sessionId": session_id, "events": [], "lagged": count })
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    write_line(
                        &out,
                        &sepia_driver::rpc::Notification::new("session.event", params),
                    );
                }
            });
            Ok(json!({ "subscribed": true, "sessionId": id }))
        }
        "events.subscribe" => {
            let (mut rx, live) = plane.subscribe_events().await;
            let out = Arc::clone(out);
            tokio::spawn(async move {
                loop {
                    let event = match rx.recv().await {
                        Ok(event) => event,
                        // The feed itself overflowed for this receiver —
                        // surface it as a lagged marker, not a gap.
                        Err(broadcast::error::RecvError::Lagged(count)) => NodeEvent::Lagged {
                            session_id: None,
                            count,
                        },
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let params = serde_json::to_value(&event)
                        .unwrap_or_else(|_| json!({ "type": "unserializable" }));
                    write_line(
                        &out,
                        &sepia_driver::rpc::Notification::new("node.event", params),
                    );
                }
            });
            // `live` is the at-subscribe snapshot; a racing attach can
            // appear both there and on the stream — dedup by sessionId.
            Ok(json!({ "subscribed": true, "live": live }))
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
        let local = Arc::new(
            build(&paths, base_options())
                .await
                .map_err(|e| CliError(format!("failed to build session plane: {e}")))?,
        );
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
                    // Requests dispatch concurrently — a blocking call
                    // (`sessions.prompt_wait`, a long `sessions.prompt`)
                    // must not starve `sessions.cancel`,
                    // `sessions.permission`, or the subscribe verbs that
                    // interleave with it. Responses are id-correlated,
                    // so completion order doesn't matter.
                    let local = Arc::clone(&local);
                    let out = Arc::clone(&out);
                    tokio::spawn(async move {
                        let response = match dispatch(&local, &req, &out).await {
                            Ok(result) => sepia_driver::rpc::Response::ok(req.id, result),
                            Err(error) => sepia_driver::rpc::Response::err(req.id, error),
                        };
                        write_line(&out, &response);
                    });
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
