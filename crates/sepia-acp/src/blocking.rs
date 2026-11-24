//! Synchronous facade over [`AcpConnection`] for consumers that can't
//! (or won't) drive a tokio runtime — kuda's summarizer, mimus's
//! semantic checker. One current-thread runtime per client; every
//! call blocks the caller until the agent answers.
//!
//! This is the unattended transport: `session/request_permission`
//! requests are auto-approved (first allow-kind option, else the
//! first option, else cancelled) — a prompt must never deadlock on a
//! permission nobody watches. Consumers needing human-in-the-loop
//! permission use the async [`AcpConnection`] API instead.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sepia_driver::rpc::{self, RpcError};
use serde_json::json;
use tokio::runtime::Runtime;

use crate::types::{
    AcpSessionUpdate, AgentSpec, ClientRequestHandler, PermissionOption, PromptPart, PromptResult,
    SpawnOptions, WireDebug,
};
use crate::{AcpConnection, spawn_agent};

/// Per-connection knobs for [`Client::connect_shell`].
#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    /// Forward the whole parent environment to the agent (default:
    /// sepia's credential-conscious allowlist). Consumers migrating
    /// from a raw `Command` spawn pass `true` to preserve prior
    /// behavior — their agents may need arbitrary auth/config vars.
    pub inherit_env: bool,
    /// Extra environment variables merged into the child's env.
    pub env: Option<BTreeMap<String, String>>,
    /// Per-call timeout — a blocked agent (wedged tool call, dead
    /// permission lane) degrades to an error instead of hanging the
    /// caller forever.
    pub call_timeout: Option<Duration>,
    /// `clientInfo.name` reported at `initialize` (the version is
    /// this crate's).
    pub client_name: Option<String>,
    /// Serve `fs/read_text_file` under this root and advertise the
    /// `fs` client capability at `initialize` — for agents that read
    /// through the client instead of shell tools.
    pub fs_root: Option<PathBuf>,
    /// ndjson transcript sink (`>` outbound, `<` inbound, pre-parse).
    /// `SEPIA_ACP_DEBUG` is the env fallback when unset.
    pub wire_debug: Option<WireDebug>,
}

/// Pick the option an unattended client grants: the first
/// allow-kind option, else the first option, else cancel (`None`).
fn auto_approve_option(options: &[PermissionOption]) -> Option<String> {
    options
        .iter()
        .find(|o| o.kind.starts_with("allow"))
        .or_else(|| options.first())
        .map(|o| o.option_id.clone())
}

/// `fs/read_text_file` handler rooted at `root` — absolute paths and
/// `..` escapes are refused; reads resolve only under the root.
pub fn fs_read_text_file_handler(root: PathBuf) -> ClientRequestHandler {
    ClientRequestHandler(Arc::new(move |method, params| {
        if method != "fs/read_text_file" {
            return Err(RpcError::new(rpc::METHOD_NOT_FOUND, "method not found"));
        }
        let rel = params["path"].as_str().unwrap_or_default();
        let path = safe_join(&root, rel)?;
        std::fs::read_to_string(&path)
            .map(|content| json!({ "content": content }))
            .map_err(|e| RpcError::new(rpc::INVALID_PARAMS, e.to_string()))
    }))
}

fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, RpcError> {
    let p = Path::new(rel);
    let escapes = rel.is_empty()
        || p.is_absolute()
        || p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir));
    if escapes {
        return Err(RpcError::new(
            rpc::INVALID_PARAMS,
            format!("fs path escapes root: {rel}"),
        ));
    }
    Ok(root.join(p))
}

/// A live ACP agent behind the synchronous facade. Drop closes the
/// connection (stdin EOF, brief grace, then kill).
pub struct Client {
    rt: Runtime,
    conn: Arc<AcpConnection>,
    /// `session/new` cwd — the repo root the agent cites paths against.
    cwd: String,
}

impl Client {
    /// Spawn `sh -c <cmd>` and run the `initialize` handshake. `cwd`
    /// is the session cwd (`session/new`); the agent process inherits
    /// the caller's working directory, matching a raw `sh -c` spawn.
    ///
    /// # Errors
    /// On spawn failure or a failed initialize.
    pub fn connect_shell(cmd: &str, cwd: &Path, options: &ClientOptions) -> Result<Self, String> {
        let name = options
            .client_name
            .clone()
            .unwrap_or_else(|| "sepia-acp".into());
        let spec = AgentSpec {
            id: name.clone(),
            label: name.clone(),
            command: vec!["sh".into(), "-c".into(), cmd.to_string()],
            env: None,
        };
        let spawn = SpawnOptions {
            cwd: ".".into(),
            env: options.env.clone(),
            model: None,
            fallbacks: None,
            inherit_env: options.inherit_env,
            client_capabilities: options
                .fs_root
                .is_some()
                .then(|| json!({ "fs": { "readTextFile": true } })),
            client_info: Some(json!({
                "name": name,
                "version": env!("CARGO_PKG_VERSION"),
            })),
            request_handler: options.fs_root.clone().map(fs_read_text_file_handler),
            wire_debug: options.wire_debug.clone(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let conn = rt
            .block_on(spawn_agent(&spec, &spawn))
            .map_err(|e| e.to_string())?;
        let conn = Arc::new(conn);
        if let Some(timeout) = options.call_timeout {
            conn.set_call_timeout(timeout);
        }
        // Unattended permission policy — answer out of band so the
        // agent never waits on a request nobody watches.
        {
            let mut permissions = conn.permissions();
            let conn = Arc::clone(&conn);
            rt.spawn(async move {
                while let Ok(request) = permissions.recv().await {
                    let option = auto_approve_option(&request.options);
                    conn.respond_to_permission(&request.request_id, option.as_deref());
                }
            });
        }
        Ok(Self {
            rt,
            conn,
            cwd: cwd.to_string_lossy().into_owned(),
        })
    }

    /// `session/new` under the connect-time cwd → the session id
    /// (empty when the agent omits `sessionId`).
    ///
    /// # Errors
    /// On RPC failure.
    pub fn new_session(&self) -> Result<String, String> {
        let conn = Arc::clone(&self.conn);
        let cwd = self.cwd.clone();
        self.rt
            .block_on(async move { conn.new_session(&cwd).await.map_err(|e| e.to_string()) })
    }

    /// `session/prompt` — resolves with the turn's [`PromptResult`]
    /// and the streamed `agent_message_chunk` text. Thought chunks and
    /// every other update kind never reach the returned text.
    ///
    /// # Errors
    /// On RPC failure or the per-call timeout.
    pub fn prompt_text(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<(PromptResult, String), String> {
        let conn = Arc::clone(&self.conn);
        let sid = session_id.to_string();
        let parts = [PromptPart::Text {
            text: text.to_string(),
        }];
        self.rt.block_on(async move {
            let mut updates = conn.updates();
            let prompt = conn.prompt(&sid, &parts);
            tokio::pin!(prompt);
            let mut out = String::new();
            let result = loop {
                tokio::select! {
                    res = &mut prompt => break res,
                    update = updates.recv() => {
                        // Lagged/Closed are cosmetic here — the prompt
                        // result is authoritative; the channel outlives
                        // the conn so Closed can't spin the loop.
                        if let Ok(AcpSessionUpdate::AgentMessageChunk { text }) = update {
                            out.push_str(&text);
                        }
                    }
                }
            };
            // Chunks dispatched just before the response can still sit
            // in the broadcast buffer — drain them.
            while let Ok(update) = updates.try_recv() {
                if let AcpSessionUpdate::AgentMessageChunk { text } = update {
                    out.push_str(&text);
                }
            }
            result.map(|r| (r, out)).map_err(|e| e.to_string())
        })
    }

    /// `session/cancel` — fire-and-forget turn cancellation.
    ///
    /// # Errors
    /// On write failure.
    pub fn cancel(&self, session_id: &str) -> Result<(), String> {
        let conn = Arc::clone(&self.conn);
        let sid = session_id.to_string();
        self.rt
            .block_on(async move { conn.cancel(&sid).await.map_err(|e| e.to_string()) })
    }

    /// Graceful close — stdin EOF, a brief exit grace, then kill.
    /// Safe to skip: `Drop` does the same.
    pub fn close(&self) {
        // Inside a tokio runtime `block_on` panics — the conn's own
        // Drop still kills the child when the client drops.
        if tokio::runtime::Handle::try_current().is_ok() {
            return;
        }
        let conn = Arc::clone(&self.conn);
        self.rt.block_on(async move { conn.close().await });
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_prefers_allow_kind() {
        let opts = vec![
            PermissionOption {
                option_id: "deny".into(),
                name: "Deny".into(),
                kind: "reject_once".into(),
            },
            PermissionOption {
                option_id: "ok".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            },
        ];
        assert_eq!(auto_approve_option(&opts).as_deref(), Some("ok"));
        // No allow-kind → first option; no options → cancel.
        assert_eq!(auto_approve_option(&opts[..1]).as_deref(), Some("deny"));
        assert_eq!(auto_approve_option(&[]), None);
    }

    #[test]
    fn fs_handler_reads_under_root_and_refuses_escape() {
        let dir = std::env::temp_dir().join(format!("sepia-acp-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        let handler = fs_read_text_file_handler(dir.clone());
        let ok = (handler.0)("fs/read_text_file", &json!({ "path": "a.txt" })).unwrap();
        assert_eq!(ok["content"], "hello");
        for bad in ["../escape", "/etc/passwd", ""] {
            assert!(
                (handler.0)("fs/read_text_file", &json!({ "path": bad })).is_err(),
                "{bad} must be refused"
            );
        }
        assert!(
            (handler.0)("fs/write_text_file", &json!({ "path": "a.txt" })).is_err(),
            "unknown methods are refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
