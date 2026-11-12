//! [`SepiaPeregrin`] — sepia's client for peregrin's entity plane.
//! Speaks `/api/entities*` on any node or hub and presents sessions:
//! `sepia.<verb>` ops for actions, snapshots for transfer, the index
//! for lists. Session-detail traffic (history, checkpoints, streams)
//! stays on the session plane — `raw_get`/`raw_post` reach it.

use serde_json::Value;

use crate::{ENTITY_KIND, entity_to_summary};

const MAX_ERROR_BODY: u64 = 64 * 1024;

/// Transport/HTTP/decode failures from the entity wire.
#[derive(Debug, thiserror::Error)]
pub enum SepiaPeregrinError {
    /// Network-level failure (DNS, connect, timeout).
    #[error("transport: {0}")]
    Transport(String),
    /// The node/hub answered non-2xx — `body` is the raw response text.
    #[error("http {status}: {body}")]
    Http {
        /// The response status code.
        status: u16,
        /// The raw response body.
        body: String,
    },
    /// The response didn't carry the shape the endpoint promises.
    #[error("malformed: {0}")]
    Malformed(String),
}

/// The entity-plane client — one node's or a hub's `/api/entities*`
/// surface. `base` is the hub/node origin; `token` the optional
/// bearer credential (hub pairing mints these).
#[derive(Clone)]
pub struct SepiaPeregrin {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl SepiaPeregrin {
    /// # Errors
    /// On an unparseable base URL.
    pub fn new(base: &str, token: Option<String>) -> Result<Self, SepiaPeregrinError> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(60)))
            .build()
            .new_agent();
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            agent,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn auth<Any>(&self, req: ureq::RequestBuilder<Any>) -> ureq::RequestBuilder<Any> {
        match &self.token {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    /// Status-check + JSON-read; non-2xx → [`SepiaPeregrinError::Http`].
    fn expect_json(res: ureq::http::Response<ureq::Body>) -> Result<Value, SepiaPeregrinError> {
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            let body = res
                .into_body()
                .into_with_config()
                .limit(MAX_ERROR_BODY)
                .read_to_string()
                .unwrap_or_default();
            return Err(SepiaPeregrinError::Http { status, body });
        }
        res.into_body()
            .read_json::<Value>()
            .map_err(|e| SepiaPeregrinError::Malformed(format!("invalid json: {e}")))
    }

    fn get(&self, path: &str) -> Result<Value, SepiaPeregrinError> {
        let res = self
            .auth(self.agent.get(self.url(path)))
            .call()
            .map_err(|e| SepiaPeregrinError::Transport(e.to_string()))?;
        Self::expect_json(res)
    }

    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, SepiaPeregrinError> {
        let res = match method {
            "POST" => self.auth(self.agent.post(self.url(path))).send_json(body),
            "PATCH" => self.auth(self.agent.patch(self.url(path))).send_json(body),
            "DELETE" => self.auth(self.agent.delete(self.url(path))).call(),
            _ => return Err(SepiaPeregrinError::Malformed(format!("verb {method}"))),
        }
        .map_err(|e| SepiaPeregrinError::Transport(e.to_string()))?;
        Self::expect_json(res)
    }

    /* ---- the entity surface ----------------------------------------- */

    /// `GET /api/entities` → session summaries (`sepia-session` docs
    /// only — other kinds on the index are skipped).
    ///
    /// # Errors
    /// On transport failure, non-2xx, or a malformed `entities` field.
    pub fn list_sessions(&self) -> Result<Vec<sepia_session::SessionSummary>, SepiaPeregrinError> {
        let body = self.get("/api/entities")?;
        let rows = body
            .get("entities")
            .and_then(Value::as_array)
            .ok_or_else(|| SepiaPeregrinError::Malformed("missing entities array".into()))?;
        Ok(rows.iter().filter_map(entity_to_summary).collect())
    }

    /// `GET /api/entities/{id}` → one session's index view.
    ///
    /// # Errors
    /// On transport failure, non-2xx, a non-`sepia-session` doc, or an
    /// indexDoc that doesn't decode.
    pub fn get_session(
        &self,
        id: &str,
    ) -> Result<sepia_session::SessionSummary, SepiaPeregrinError> {
        let doc = self.get(&format!("/api/entities/{id}"))?;
        entity_to_summary(&doc)
            .ok_or_else(|| SepiaPeregrinError::Malformed(format!("entity {id} is not a session")))
    }

    /// `POST /api/entities` — create a session (`indexDoc` is the
    /// create body the session plane understands: `cwd`, `agent`,
    /// `title`, …). Returns the new session's id.
    ///
    /// # Errors
    /// On transport failure, non-2xx, or a missing `id` in the reply.
    pub fn create_session(&self, init: &Value) -> Result<String, SepiaPeregrinError> {
        let body = serde_json::json!({"kind": ENTITY_KIND, "indexDoc": init});
        let res = self.send("POST", "/api/entities", &body)?;
        res.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SepiaPeregrinError::Malformed("create returned no id".into()))
    }

    /// `POST /api/entities/{id}/ops` — one `sepia.<verb>` op.
    ///
    /// # Errors
    /// On transport failure or non-2xx (unknown verbs 501 node-side).
    pub fn session_op(
        &self,
        id: &str,
        verb: &str,
        payload: &Value,
    ) -> Result<(), SepiaPeregrinError> {
        let body = serde_json::json!({
            "kind": format!("sepia.{verb}"),
            "payload": payload,
        });
        self.send("POST", &format!("/api/entities/{id}/ops"), &body)
            .map(|_| ())
    }

    /// `sepia.prompt` — queue a turn. `parts` is the prompt content
    /// (string or content-part array, per the session wire).
    ///
    /// # Errors
    /// See [`Self::session_op`].
    pub fn prompt(&self, id: &str, parts: &Value) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "prompt", &serde_json::json!({"parts": parts}))
    }

    /// `sepia.prompt_wait` — prompt and block until the turn reaches a
    /// terminal state. Returns the `PromptWaitResult` document
    /// (`stopReason`, `text`, `eventCount`, `durationMs`, `timedOut`,
    /// `error`). `timeout_ms` `None`/`0` waits without limit; an
    /// elapsed timeout cancels the turn.
    ///
    /// # Errors
    /// On transport failure or non-2xx — turn-level failures arrive in
    /// the result's `error` field instead.
    pub fn prompt_wait(
        &self,
        id: &str,
        parts: &Value,
        timeout_ms: Option<u64>,
    ) -> Result<Value, SepiaPeregrinError> {
        let mut payload = serde_json::json!({"parts": parts});
        if let Some(ms) = timeout_ms {
            payload["timeoutMs"] = serde_json::json!(ms);
        }
        let body = serde_json::json!({
            "kind": "sepia.prompt_wait",
            "payload": payload,
        });
        self.send("POST", &format!("/api/entities/{id}/ops"), &body)
    }

    /// `sepia.attach` / `sepia.detach` — live-driver attach/release.
    /// `takeover` asks the node to break a foreign hold.
    ///
    /// # Errors
    /// See [`Self::session_op`].
    pub fn attach(&self, id: &str, takeover: bool) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "attach", &serde_json::json!({"takeover": takeover}))
    }

    /// # Errors
    /// See [`Self::session_op`].
    pub fn detach(&self, id: &str) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "detach", &serde_json::json!({}))
    }

    /// # Errors
    /// See [`Self::session_op`].
    pub fn cancel(&self, id: &str) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "cancel", &serde_json::json!({}))
    }

    /// `sepia.permission` — answer an open permission request.
    ///
    /// # Errors
    /// See [`Self::session_op`].
    pub fn permission(&self, id: &str, answer: &Value) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "permission", answer)
    }

    /// `sepia.meta` — overlay patch (title/pin/archive/projectIds).
    ///
    /// # Errors
    /// See [`Self::session_op`].
    pub fn meta_patch(&self, id: &str, patch: &Value) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "meta", patch)
    }

    /// `sepia.restore` / `sepia.rewind` / `sepia.convert` — store-time
    /// actions. `params` is the verb's wire body (`checkpoint`, `keep`,
    /// `target`, …).
    ///
    /// # Errors
    /// See [`Self::session_op`].
    pub fn restore(&self, id: &str, checkpoint: &str) -> Result<(), SepiaPeregrinError> {
        self.session_op(
            id,
            "restore",
            &serde_json::json!({"checkpoint": checkpoint}),
        )
    }

    /// # Errors
    /// See [`Self::session_op`].
    pub fn rewind(&self, id: &str, params: &Value) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "rewind", params)
    }

    /// # Errors
    /// See [`Self::session_op`].
    pub fn convert(&self, id: &str, target: &str) -> Result<(), SepiaPeregrinError> {
        self.session_op(id, "convert", &serde_json::json!({"target": target}))
    }

    /// `entity.delete` — the substrate's delete op.
    ///
    /// # Errors
    /// See [`Self::session_op`].
    pub fn delete_session(&self, id: &str) -> Result<(), SepiaPeregrinError> {
        let body = serde_json::json!({"kind": "entity.delete", "payload": {}});
        self.send("POST", &format!("/api/entities/{id}/ops"), &body)
            .map(|_| ())
    }

    /// `GET /api/entities/{id}/snapshot` — the session IR for transfer.
    ///
    /// # Errors
    /// On transport failure or non-2xx.
    pub fn snapshot_export(&self, id: &str) -> Result<Value, SepiaPeregrinError> {
        self.get(&format!("/api/entities/{id}/snapshot"))
    }

    /// `POST /api/entities/snapshot` — import a snapshot on this node.
    /// Returns the new session's entity id.
    ///
    /// # Errors
    /// On transport failure, non-2xx, or a missing `id`.
    pub fn snapshot_import(&self, body: &Value) -> Result<String, SepiaPeregrinError> {
        let res = self.send("POST", "/api/entities/snapshot", body)?;
        res.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SepiaPeregrinError::Malformed("import returned no id".into()))
    }

    /* ---- session-detail passthrough -------------------------------- */

    /// The session plane's detail endpoints — history, checkpoints,
    /// streams — are sepia's own surface; `raw_get`/`raw_post` reach
    /// `/api/sessions/{id}/<path>` (or any absolute `/api/*` path)
    /// through the same auth.
    ///
    /// # Errors
    /// On transport failure or non-2xx.
    pub fn raw_get(&self, path: &str) -> Result<Value, SepiaPeregrinError> {
        self.get(path)
    }

    /// # Errors
    /// On transport failure or non-2xx.
    pub fn raw_post(&self, path: &str, body: &Value) -> Result<Value, SepiaPeregrinError> {
        self.send("POST", path, body)
    }
}
