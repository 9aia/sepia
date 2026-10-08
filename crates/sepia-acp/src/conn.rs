//! `AcpConnection` — a spawned ACP agent: ndjson JSON-RPC over stdio,
//! pending-request tracking, update/permission dispatch, kill-on-drop.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sepia_driver_sdk::rpc::{self, Inbound, RpcError};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast, oneshot};

use crate::broker::PermissionBroker;
use crate::normalize::as_record;
use crate::registry::model_args;
use crate::stderr::StderrTail;
use crate::types::{
    AcpCapabilities, AcpSessionInfo, AcpSessionUpdate, AgentSpec, PermissionRequest, PromptPart,
    SpawnOptions,
};

const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const PROTOCOL_VERSION: u64 = 1;

const METHOD_UPDATE: &str = "session/update";
const METHOD_REQUEST_PERMISSION: &str = "session/request_permission";

/// Environment variables a spawned agent needs; everything else is
/// dropped (the allowlist keeps credentials out of the child).
const ALLOWED_ENV_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "TERM",
    "WINDSURF_API_KEY",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// `SEPIA_INHERIT_ENV=1` opts back into forwarding the whole parent env.
fn build_child_env(spec: &AgentSpec, options: &SpawnOptions) -> Vec<(String, String)> {
    let inherit = std::env::var("SEPIA_INHERIT_ENV").is_ok_and(|v| v == "1");
    let mut env = std::collections::BTreeMap::new();
    if inherit {
        env.extend(std::env::vars());
    } else {
        for key in ALLOWED_ENV_KEYS {
            if let Ok(value) = std::env::var(key) {
                env.insert((*key).to_string(), value);
            }
        }
    }
    if let Some(spec_env) = &spec.env {
        env.extend(spec_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(opt_env) = &options.env {
        env.extend(opt_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    env.into_iter().collect()
}

struct Inner {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>,
    next_id: AtomicU64,
    updates: broadcast::Sender<AcpSessionUpdate>,
    permissions_tx: broadcast::Sender<PermissionRequest>,
    broker: PermissionBroker,
    exit_error: OnceCell<RpcError>,
    exited: AtomicBool,
}

impl Inner {
    async fn dispatch(self: &Arc<Self>, line: &str) {
        match rpc::decode_line(line) {
            Ok(Inbound::Response(response)) => {
                let sender = self.pending.lock().await.remove(&response.id);
                if let Some(tx) = sender {
                    let result = match (response.result, response.error) {
                        (_, Some(error)) => Err(error),
                        (result, None) => Ok(result.unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(result);
                }
            }
            Ok(Inbound::Notification(note)) => {
                if note.method == METHOD_UPDATE {
                    let update = crate::normalize::normalize_update(&note.params["update"]);
                    let _ = self.updates.send(update);
                }
            }
            Ok(Inbound::Request(req)) => {
                if req.method == METHOD_REQUEST_PERMISSION {
                    // Out of band: a permission waits for a human — the
                    // reader must keep dispatching meanwhile (the request
                    // response and later lines depend on it).
                    let inner = Arc::clone(self);
                    tokio::spawn(async move { inner.answer_permission(req).await });
                }
            }
            Err(_) => {}
        }
    }

    async fn answer_permission(&self, req: rpc::Request) {
        let (request, rx) = self.broker.begin(&req.params);
        let _ = self.permissions_tx.send(request);
        let outcome = rx.await.unwrap_or_else(|_| Err("closed".into()));
        let response = match outcome {
            Ok(result) => rpc::Response::ok(req.id, result),
            Err(e) => rpc::Response::err(req.id, RpcError::internal(e)),
        };
        if let Ok(mut line) = rpc::encode_line(&response) {
            line.push('\n');
            if let Some(stdin) = &mut *self.stdin.lock().await {
                let _ = stdin.write_all(line.as_bytes()).await;
                let _ = stdin.flush().await;
            }
        }
    }

    async fn write(&self, line: &str) -> Result<(), RpcError> {
        let mut guard = self.stdin.lock().await;
        let Some(stdin) = &mut *guard else {
            return Err(RpcError::internal("agent stdin closed"));
        };
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| RpcError::internal(format!("agent stdin: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| RpcError::internal(format!("agent stdin: {e}")))
    }

    async fn fail_all(&self, reason: &str) {
        self.exited.store(true, Ordering::SeqCst);
        let _ = self.exit_error.set(RpcError::internal(reason.to_string()));
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(RpcError::internal(reason)));
        }
        self.broker.fail_all(reason);
    }
}

/// A live ACP agent connection. Drop kills the child.
pub struct AcpConnection {
    inner: Arc<Inner>,
    stderr: Arc<StderrTail>,
    capabilities: RwLock<AcpCapabilities>,
}

impl AcpConnection {
    /// Subscribe to normalized session updates.
    pub fn updates(&self) -> broadcast::Receiver<AcpSessionUpdate> {
        self.inner.updates.subscribe()
    }

    /// Subscribe to permission requests.
    pub fn permissions(&self) -> broadcast::Receiver<PermissionRequest> {
        self.inner.permissions_tx.subscribe()
    }

    /// The capability set captured at `initialize` — defaults to "nothing
    /// advertised" before that resolves.
    pub async fn capabilities(&self) -> AcpCapabilities {
        self.capabilities.read().await.clone()
    }

    /// Bounded, prefixed tail of the agent's stderr for error reporting.
    pub fn recent_stderr(&self) -> Vec<String> {
        self.stderr.recent()
    }

    /// Settles a pending permission request; returns false when the id is
    /// unknown. `None` selects "cancelled".
    pub fn respond_to_permission(&self, request_id: &str, option_id: Option<&str>) -> bool {
        self.inner.broker.respond(request_id, option_id)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if let Some(err) = self.inner.exit_error.get() {
            return Err(err.clone());
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);
        let request = rpc::Request::new(id, method, params);
        let line =
            rpc::encode_line(&request).map_err(|e| RpcError::internal(e.to_string()))? + "\n";
        if let Err(e) = self.inner.write(&line).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::internal("agent response channel closed")),
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                Err(RpcError::internal(format!("agent call {method} timed out")))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let note = rpc::Notification::new(method, params);
        let line = rpc::encode_line(&note).map_err(|e| RpcError::internal(e.to_string()))? + "\n";
        self.inner.write(&line).await
    }

    /// The `initialize` handshake — must run before the connection is
    /// handed out.
    pub async fn initialize(&self) -> Result<(), RpcError> {
        let result = self
            .call(
                "initialize",
                json!({ "protocolVersion": PROTOCOL_VERSION, "clientCapabilities": {} }),
            )
            .await?;
        let caps = as_record(&result["agentCapabilities"]);
        let prompt = as_record(&caps["promptCapabilities"]);
        let session = as_record(&caps["sessionCapabilities"]);
        // Each session method is advertised by a (possibly empty) entry
        // object — presence is the capability.
        let present = |key: &str| !session.get(key).is_none_or(Value::is_null);
        *self.capabilities.write().await = AcpCapabilities {
            load_session: caps.get("loadSession") == Some(&Value::Bool(true)),
            session_list: present("list"),
            prompt_capabilities: crate::types::AcpPromptCapabilities {
                image: prompt.get("image") == Some(&Value::Bool(true)),
                audio: prompt.get("audio") == Some(&Value::Bool(true)),
                embedded_context: prompt.get("embeddedContext") == Some(&Value::Bool(true)),
            },
            session_capabilities: crate::types::AcpSessionCapabilities {
                list: present("list"),
                delete: present("delete"),
                fork: present("fork"),
                resume: present("resume"),
                close: present("close"),
                additional_directories: present("additionalDirectories"),
            },
        };
        Ok(())
    }

    /// `session/list` → flattened session info with devin's lock metadata.
    ///
    /// # Errors
    /// On RPC failure or a non-responsive agent.
    pub async fn list_sessions(&self) -> Result<Vec<AcpSessionInfo>, RpcError> {
        let result = self.call("session/list", json!({})).await?;
        Ok(crate::normalize::as_array(&result["sessions"])
            .iter()
            .map(|entry| {
                let s = as_record(entry);
                let meta = as_record(crate::normalize::field(s, "_meta"));
                AcpSessionInfo {
                    session_id: crate::normalize::as_string(crate::normalize::field(
                        s,
                        "sessionId",
                    ))
                    .unwrap_or_default()
                    .to_string(),
                    cwd: crate::normalize::as_string(crate::normalize::field(s, "cwd"))
                        .unwrap_or_default()
                        .to_string(),
                    title: crate::normalize::as_string(crate::normalize::field(s, "title"))
                        .unwrap_or_default()
                        .to_string(),
                    updated_at: crate::normalize::as_string(crate::normalize::field(
                        s,
                        "updatedAt",
                    ))
                    .unwrap_or_default()
                    .to_string(),
                    locked: meta.get("cognition.ai/isLocked") == Some(&Value::Bool(true)),
                    lock_holder_pid: crate::normalize::as_number_or_null(crate::normalize::field(
                        meta,
                        "cognition.ai/lockHolderPid",
                    )),
                }
            })
            .collect())
    }

    /// `session/new` → the new session's id.
    ///
    /// # Errors
    /// On RPC failure.
    pub async fn new_session(&self, cwd: &str) -> Result<String, RpcError> {
        let result = self
            .call("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await?;
        Ok(result["sessionId"].as_str().unwrap_or_default().to_string())
    }

    /// `session/load` — resume a stored session.
    ///
    /// # Errors
    /// On RPC failure.
    pub async fn load_session(&self, session_id: &str, cwd: &str) -> Result<(), RpcError> {
        self.call(
            "session/load",
            json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": [] }),
        )
        .await?;
        Ok(())
    }

    /// `session/prompt` — send a prompt; streaming replies arrive via
    /// [`Self::updates`].
    ///
    /// # Errors
    /// On RPC failure.
    pub async fn prompt(&self, session_id: &str, parts: &[PromptPart]) -> Result<(), RpcError> {
        self.call(
            "session/prompt",
            json!({ "sessionId": session_id, "prompt": parts }),
        )
        .await?;
        Ok(())
    }

    /// `session/cancel` — fire-and-forget turn cancellation.
    ///
    /// # Errors
    /// On write failure.
    pub async fn cancel(&self, session_id: &str) -> Result<(), RpcError> {
        self.notify("session/cancel", json!({ "sessionId": session_id }))
            .await
    }

    /// `session/delete` — delete the session from the agent's store.
    ///
    /// # Errors
    /// On RPC failure or agent refusal.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), RpcError> {
        self.call("session/delete", json!({ "sessionId": session_id }))
            .await?;
        Ok(())
    }

    /// Close stdin (agents exit on EOF), give it `EXIT_TIMEOUT`, then kill.
    pub async fn close(self: Arc<Self>) {
        *self.inner.stdin.lock().await = None;
        let deadline = tokio::time::Instant::now() + EXIT_TIMEOUT;
        loop {
            {
                let mut child = self.inner.child.lock().await;
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => return,
                    Ok(None) if tokio::time::Instant::now() >= deadline => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        return;
                    }
                    Ok(None) => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Spawn `spec` + `options` and run the `initialize` handshake.
///
/// # Errors
/// On spawn failure, missing stdio, or a failed initialize.
pub async fn spawn_agent(
    spec: &AgentSpec,
    options: &SpawnOptions,
) -> Result<AcpConnection, RpcError> {
    let Some((command, args)) = spec.command.split_first() else {
        return Err(RpcError::internal(format!(
            "Agent \"{}\" has no command to spawn",
            spec.id
        )));
    };
    let spawn_args: Vec<String> = args
        .iter()
        .cloned()
        .chain(model_args(
            &spec.id,
            options.model.as_deref(),
            options.fallbacks.as_deref(),
        ))
        .collect();
    let mut child = Command::new(command)
        .args(&spawn_args)
        .current_dir(if options.cwd.is_empty() {
            "."
        } else {
            &options.cwd
        })
        .env_clear()
        .envs(build_child_env(spec, options))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| RpcError::internal(format!("Agent \"{}\" failed to spawn: {e}", spec.id)))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RpcError::internal("agent stdout missing"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| RpcError::internal("agent stdin missing"))?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| RpcError::internal("agent stderr missing"))?;

    let stderr = Arc::new(StderrTail::new(
        &spec.id,
        std::env::var("SEPIA_DEBUG").is_ok_and(|v| v == "1"),
    ));

    let (updates, _) = broadcast::channel(512);
    let (permissions_tx, _) = broadcast::channel(128);
    let inner = Arc::new(Inner {
        child: Mutex::new(child),
        stdin: Mutex::new(Some(stdin)),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        updates,
        permissions_tx,
        broker: PermissionBroker::new(),
        exit_error: OnceCell::new(),
        exited: AtomicBool::new(false),
    });

    // stdout reader → dispatch.
    {
        let inner = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                inner.dispatch(&line).await;
            }
            inner.fail_all("agent stdout closed").await;
        });
    }
    // stderr reader → ring tail.
    {
        let stderr = Arc::clone(&stderr);
        tokio::spawn(async move {
            let mut lines = BufReader::new(child_stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&format!("{line}\n"));
            }
        });
    }
    // exit watcher → fail pending + permissions.
    {
        let inner = Arc::clone(&inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut child = inner.child.lock().await;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        drop(child);
                        inner
                            .fail_all(&format!(
                                "Agent process exited before responding ({status})"
                            ))
                            .await;
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
        });
    }

    let conn = AcpConnection {
        inner,
        stderr,
        capabilities: RwLock::new(AcpCapabilities::default()),
    };
    conn.initialize().await?;
    Ok(conn)
}
