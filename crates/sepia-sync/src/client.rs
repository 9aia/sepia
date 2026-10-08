//! `NodeClient` — the hub→node HTTP client (protocol v2).
//!
//! Blocking on purpose (`ureq`): the sync engine runs it on
//! `spawn_blocking` threads. Carries a per-node bearer token, reads
//! `GET /api/sessions*` responses as raw `Value`s (the projection stores
//! the wire rows verbatim), posts session-scoped ops, and tails the
//! `/api/events` node feed as a `data: {json}` SSE line reader.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use sepia_proto::SessionEvent;
use serde_json::Value;
use ureq::http::Response;
use ureq::{Agent, Body, BodyReader, RequestBuilder};

/// Per-request budgets. `sse_max_age` doubles as the cancellation bound
/// for the SSE pump thread: ureq's `recv_body` timeout is a total
/// body-phase deadline, so the stream is cut at most that long after it
/// opens (heartbeats wake the reader far sooner on a healthy node).
#[derive(Clone, Copy, Debug)]
pub struct ClientConfig {
    /// TCP connect (+ TLS handshake) budget.
    pub connect_timeout: Duration,
    /// End-to-end budget for plain REST calls.
    pub request_timeout: Duration,
    /// Max lifetime of one SSE stream; the engine reconnects and
    /// re-syncs when it lapses.
    pub sse_max_age: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(15),
            sse_max_age: Duration::from_secs(60),
        }
    }
}

/// Failures a node call can produce. [`NodeError::is_transport`] splits
/// "the node didn't answer" (retry/mark down) from "the node answered
/// with an error" (the write was rejected — dead-letter, don't reorder).
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// Non-2xx status; `body` is the node's `{error, code?}` payload.
    #[error("http {status}: {body}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated).
        body: String,
    },
    /// Connect/IO/timeout — the node is unreachable or dropped us.
    #[error("transport: {0}")]
    Transport(#[source] ureq::Error),
    /// Read failure mid-body (SSE stream).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Response wasn't the JSON shape the API guarantees.
    #[error("malformed response: {0}")]
    Malformed(String),
    /// `post_op` was given an op name with no endpoint mapping.
    #[error("unsupported op {0:?}")]
    Unsupported(String),
    /// A `spawn_blocking` worker panicked or was aborted.
    #[error("task: {0}")]
    Task(String),
}

impl NodeError {
    /// `true` when the request never got a meaningful answer — the node
    /// should be treated as down and writes queued.
    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::Io(_))
    }
}

/// `GET /api/sessions/{id}/history` page — messages stay opaque `Value`s
/// (history is node-fetched on demand; the hub only caches it).
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPage {
    pub messages: Vec<Value>,
    pub total: usize,
    /// Absolute index of `messages[0]`; `> 0` means earlier history exists.
    pub start: usize,
}

/// Blocking client for one paired node.
#[derive(Clone)]
pub struct NodeClient {
    agent: Agent,
    /// Base URL without a trailing slash (`http://host:port`).
    base: String,
    token: Option<String>,
    config: ClientConfig,
}

/// Error bodies are only for diagnostics — never let them be huge.
const MAX_ERROR_BODY: u64 = 64 * 1024;

impl NodeClient {
    /// # Errors
    /// On an unusable base URL.
    pub fn new(base: &str, token: Option<String>) -> Result<Self, NodeError> {
        Self::with_config(base, token, ClientConfig::default())
    }

    /// # Errors
    /// On an unusable base URL.
    pub fn with_config(
        base: &str,
        token: Option<String>,
        config: ClientConfig,
    ) -> Result<Self, NodeError> {
        let agent = Agent::new_with_config(
            Agent::config_builder()
                .timeout_global(Some(config.request_timeout))
                .timeout_connect(Some(config.connect_timeout))
                // We check statuses ourselves — a 4xx body carries the
                // node's `{error, code}` and feeds dead-letter context.
                .http_status_as_error(false)
                .max_redirects(0)
                .build(),
        );
        let base = base.trim_end_matches('/');
        if !base.starts_with("http://") && !base.starts_with("https://") {
            return Err(NodeError::Malformed(format!(
                "node url must start with http(s):// — got {base:?}"
            )));
        }
        Ok(Self {
            agent,
            base: base.to_string(),
            token,
            config,
        })
    }

    fn auth<Any>(&self, req: RequestBuilder<Any>) -> RequestBuilder<Any> {
        match &self.token {
            Some(token) => req.header("authorization", format!("Bearer {token}")),
            None => req,
        }
    }

    fn query_opt<Any>(
        req: RequestBuilder<Any>,
        key: &str,
        value: Option<&str>,
    ) -> RequestBuilder<Any> {
        match value {
            Some(v) => req.query(key, v),
            None => req,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn session_url(&self, id: &str, suffix: &str) -> String {
        let mut url = format!("{}/api/sessions/{}", self.base, encode_segment(id));
        if !suffix.is_empty() {
            url.push('/');
            url.push_str(suffix);
        }
        url
    }

    /// Check the status and read a JSON body; non-2xx → [`NodeError::Http`].
    fn expect_json(res: Response<Body>) -> Result<Value, NodeError> {
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            let body = res
                .into_body()
                .into_with_config()
                .limit(MAX_ERROR_BODY)
                .read_to_string()
                .unwrap_or_default();
            return Err(NodeError::Http { status, body });
        }
        res.into_body()
            .read_json::<Value>()
            .map_err(|e| NodeError::Malformed(format!("invalid json body: {e}")))
    }

    /// `GET /api/health` — cheap liveness probe (auth-exempt on the node).
    ///
    /// # Errors
    /// On transport failure or non-2xx.
    pub fn health(&self) -> Result<(), NodeError> {
        let res = self
            .auth(self.agent.get(self.url("/api/health")))
            .call()
            .map_err(NodeError::Transport)?;
        let status = res.status().as_u16();
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Self::expect_json(res).map(|_| ())
        }
    }

    /// `GET /api/sessions` — the raw summary rows (`{sessions: [...]}`),
    /// meta overlay already applied node-side. `with_locks` adds the
    /// `withLocks=1` lock-probe fields.
    ///
    /// # Errors
    /// On transport failure, non-2xx, or a non-array `sessions` field.
    pub fn list_sessions(&self, with_locks: bool) -> Result<Vec<Value>, NodeError> {
        let mut req = self.auth(self.agent.get(self.url("/api/sessions")));
        if with_locks {
            req = req.query("withLocks", "1");
        }
        let res = req.call().map_err(NodeError::Transport)?;
        let body = Self::expect_json(res)?;
        match body.get("sessions").and_then(Value::as_array) {
            Some(sessions) => Ok(sessions.clone()),
            None => Err(NodeError::Malformed(
                "GET /api/sessions: missing sessions array".into(),
            )),
        }
    }

    /// `GET /api/sessions/{id}` — one summary row (`?agent` disambiguates
    /// colliding ids across agent stores).
    ///
    /// # Errors
    /// On transport failure, non-2xx, or a non-object body.
    pub fn get_summary(&self, id: &str, agent: Option<&str>) -> Result<Value, NodeError> {
        let req = self.auth(self.agent.get(self.session_url(id, "")));
        let res = Self::query_opt(req, "agent", agent)
            .call()
            .map_err(NodeError::Transport)?;
        Self::expect_json(res)
    }

    /// `GET /api/sessions/{id}/history?limit&before` — paged message
    /// bodies, fetched on demand only.
    ///
    /// # Errors
    /// On transport failure, non-2xx, or a malformed page.
    pub fn get_history(
        &self,
        id: &str,
        limit: Option<usize>,
        before: Option<i64>,
        agent: Option<&str>,
    ) -> Result<HistoryPage, NodeError> {
        let mut req = self.auth(self.agent.get(self.session_url(id, "history")));
        req = Self::query_opt(req, "agent", agent);
        if let Some(limit) = limit {
            req = req.query("limit", limit.to_string());
        }
        if let Some(before) = before {
            req = req.query("before", before.to_string());
        }
        let res = req.call().map_err(NodeError::Transport)?;
        let body = Self::expect_json(res)?;
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| NodeError::Malformed("history: missing messages".into()))?;
        Ok(HistoryPage {
            messages,
            total: body
                .get("total")
                .and_then(Value::as_u64)
                .ok_or_else(|| NodeError::Malformed("history: missing total".into()))?
                as usize,
            start: body
                .get("start")
                .and_then(Value::as_u64)
                .ok_or_else(|| NodeError::Malformed("history: missing start".into()))?
                as usize,
        })
    }

    /// Post a session-scoped write. `op` is the outbox op name:
    /// `prompt`, `cancel`, `permission`, `meta.patch`/`meta`, `delete`.
    ///
    /// # Errors
    /// [`NodeError::Unsupported`] for unknown ops; transport/Http else.
    pub fn post_op(
        &self,
        session_id: &str,
        agent: Option<&str>,
        op: &str,
        payload: &Value,
    ) -> Result<Value, NodeError> {
        let res = match op {
            "prompt" => Self::query_opt(
                self.auth(self.agent.post(self.session_url(session_id, "prompt"))),
                "agent",
                agent,
            )
            .send_json(payload),
            "cancel" => Self::query_opt(
                self.auth(self.agent.post(self.session_url(session_id, "cancel"))),
                "agent",
                agent,
            )
            .send_empty(),
            "permission" => Self::query_opt(
                self.auth(self.agent.post(self.session_url(session_id, "permission"))),
                "agent",
                agent,
            )
            .send_json(payload),
            "meta.patch" | "meta" => Self::query_opt(
                self.auth(self.agent.patch(self.session_url(session_id, ""))),
                "agent",
                agent,
            )
            .send_json(payload),
            "delete" => Self::query_opt(
                self.auth(self.agent.delete(self.session_url(session_id, ""))),
                "agent",
                agent,
            )
            .call(),
            other => return Err(NodeError::Unsupported(other.to_string())),
        }
        .map_err(NodeError::Transport)?;
        Self::expect_json(res)
    }

    /// `GET /api/events` — the node-level feed. The returned reader
    /// yields [`FeedEvent`]s until EOF or the `sse_max_age` body deadline
    /// trips (then `next_event` errors and the caller reconnects).
    ///
    /// # Errors
    /// On connect failure or a non-2xx status.
    pub fn events(&self) -> Result<EventStream<BodyReader<'static>>, NodeError> {
        let req = self.auth(self.agent.get(self.url("/api/events")));
        self.open_stream(req)
    }

    /// `GET /api/sessions/{id}/stream` — a session's live `SessionEvent`
    /// frames (plus `lagged` markers). Same SSE reader as [`Self::events`].
    ///
    /// # Errors
    /// On connect failure or a non-2xx status.
    pub fn session_stream(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<EventStream<BodyReader<'static>>, NodeError> {
        let req = self.auth(self.agent.get(self.session_url(id, "stream")));
        let req = Self::query_opt(req, "agent", agent);
        self.open_stream(req)
    }

    /// The SSE request gets its own timeout profile: no global deadline
    /// (the stream is open-ended), a bounded header wait, and the
    /// `sse_max_age` body budget as the read deadline.
    fn open_stream(
        &self,
        req: RequestBuilder<ureq::typestate::WithoutBody>,
    ) -> Result<EventStream<BodyReader<'static>>, NodeError> {
        let res = req
            .config()
            .timeout_global(None)
            .timeout_recv_response(Some(self.config.request_timeout))
            .timeout_recv_body(Some(self.config.sse_max_age))
            .timeout_connect(Some(self.config.connect_timeout))
            .build()
            .call()
            .map_err(NodeError::Transport)?;
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            let body = res
                .into_body()
                .into_with_config()
                .limit(MAX_ERROR_BODY)
                .read_to_string()
                .unwrap_or_default();
            return Err(NodeError::Http { status, body });
        }
        Ok(EventStream::from_reader(res.into_body().into_reader()))
    }
}

/// One parsed SSE frame off a node stream.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedEvent {
    /// `event: session|meta|project` — `{id, agent?, patch}` summary diff.
    Diff {
        /// Feed kind (`session`, `meta`, `project`).
        kind: String,
        /// Session (or project) id the patch applies to.
        id: String,
        /// Agent hint on `session`/`meta` payloads.
        agent: Option<String>,
        /// The diff body — `{"deleted": true}` is the tombstone.
        patch: Value,
    },
    /// `event: lagged` — the subscription dropped frames; resync.
    Lagged,
    /// `event: heartbeat` — node keep-alive.
    Heartbeat,
    /// A typed `SessionEvent` (`data:` frames on a session stream).
    Session(SessionEvent),
    /// Anything else — forward-compatible ignore.
    Other {
        /// SSE `event:` name ("" when absent).
        event: String,
        /// Parsed `data:` payload (`null` when unparseable).
        data: Value,
    },
}

/// Blocking `data: {json}` SSE reader — accumulates `event:`/`data:`
/// lines until a blank line dispatches the frame. Comment lines
/// (`: ping` keep-alives) are skipped.
pub struct EventStream<R: std::io::Read> {
    reader: BufReader<R>,
    event: Option<String>,
    data: String,
}

impl<R: std::io::Read> EventStream<R> {
    /// Wrap any reader — the crate's own constructor for
    /// non-ureq transports (tests, in-memory frames).
    pub fn from_reader(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            event: None,
            data: String::new(),
        }
    }

    /// Next frame, blocking. `Ok(None)` on clean EOF; a partial pending
    /// frame at EOF is dispatched before `None` (the server's last write
    /// may not end with a blank line).
    ///
    /// # Errors
    /// On read failure — the caller treats it as a dropped connection.
    pub fn next_event(&mut self) -> Result<Option<FeedEvent>, NodeError> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    if self.data.is_empty() && self.event.is_none() {
                        return Ok(None);
                    }
                    return Ok(Some(self.dispatch()));
                }
                Ok(_) => {
                    let line = line.trim_end_matches(['\n', '\r']);
                    if line.is_empty() {
                        if self.data.is_empty() && self.event.is_none() {
                            continue;
                        }
                        return Ok(Some(self.dispatch()));
                    }
                    if line.starts_with(':') {
                        continue;
                    }
                    if let Some(rest) = line.strip_prefix("data:") {
                        if !self.data.is_empty() {
                            self.data.push('\n');
                        }
                        self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                    } else if let Some(rest) = line.strip_prefix("event:") {
                        self.event = Some(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                    }
                    // `id:` / `retry:` / unknown fields — ignored.
                }
                Err(e) => return Err(NodeError::Io(e)),
            }
        }
    }

    fn dispatch(&mut self) -> FeedEvent {
        let event = self.event.take().unwrap_or_default();
        let data = std::mem::take(&mut self.data);
        classify(&event, &data)
    }
}

/// Frame → [`FeedEvent`]. `session`/`meta`/`project` frames carry the
/// `{id, agent?, patch}` diff shape; unnamed/`message` frames are parsed
/// as typed `SessionEvent`s (the session-stream wire).
fn classify(event: &str, data: &str) -> FeedEvent {
    let parsed: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    match event {
        "heartbeat" => FeedEvent::Heartbeat,
        "lagged" => FeedEvent::Lagged,
        kind @ ("session" | "meta" | "project") => {
            let id = parsed.get("id").and_then(Value::as_str);
            match id {
                Some(id) => FeedEvent::Diff {
                    kind: kind.to_string(),
                    id: id.to_string(),
                    agent: parsed
                        .get("agent")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    patch: parsed.get("patch").cloned().unwrap_or(Value::Null),
                },
                None => FeedEvent::Other {
                    event: kind.to_string(),
                    data: parsed,
                },
            }
        }
        _ => match serde_json::from_str::<SessionEvent>(data) {
            Ok(session_event) => FeedEvent::Session(session_event),
            Err(_) => FeedEvent::Other {
                event: event.to_string(),
                data: parsed,
            },
        },
    }
}

/// RFC 3986 unreserved characters pass through; everything else is
/// percent-encoded — session ids are opaque and may carry `:`/` `/etc.
fn encode_segment(segment: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frames(input: &str) -> Vec<FeedEvent> {
        let mut stream = EventStream::from_reader(input.as_bytes());
        let mut out = Vec::new();
        loop {
            match stream.next_event() {
                Ok(Some(ev)) => out.push(ev),
                Ok(None) => break,
                Err(e) => panic!("next_event failed: {e}"),
            }
        }
        out
    }

    #[test]
    fn parses_node_feed_frames() {
        let input = concat!(
            "event: session\n",
            "data: {\"id\":\"s1\",\"agent\":\"devin\",\"patch\":{\"busy\":true}}\n",
            "\n",
            ": ping\n",
            "event: heartbeat\n",
            "data: {\"ts\":1}\n",
            "\n",
            "event: meta\n",
            "data: {\"id\":\"s1\",\"patch\":{\"title\":\"T\"}}\n",
            "\n",
            "event: lagged\n",
            "data: {}\n",
            "\n"
        );
        let got = frames(input);
        assert_eq!(got.len(), 4);
        assert_eq!(
            got[0],
            FeedEvent::Diff {
                kind: "session".into(),
                id: "s1".into(),
                agent: Some("devin".into()),
                patch: json!({"busy": true}),
            }
        );
        assert_eq!(got[1], FeedEvent::Heartbeat);
        assert_eq!(
            got[2],
            FeedEvent::Diff {
                kind: "meta".into(),
                id: "s1".into(),
                agent: None,
                patch: json!({"title": "T"}),
            }
        );
        assert_eq!(got[3], FeedEvent::Lagged);
    }

    #[test]
    fn parses_bare_data_frames_as_session_events() {
        // The real wire shape: tag is camelCase, fields snake_case.
        let event = SessionEvent::RunStarted {
            thread_id: "s1".into(),
            run_id: "r1".into(),
        };
        let input = format!("data: {}\n\n", serde_json::to_string(&event).unwrap());
        let got = frames(&input);
        assert_eq!(
            got,
            vec![FeedEvent::Session(SessionEvent::RunStarted {
                thread_id: "s1".into(),
                run_id: "r1".into(),
            })]
        );
    }

    #[test]
    fn multi_line_data_and_eof_flush() {
        // Multi-line `data:` joins with \n; a pending frame at EOF is
        // still dispatched (no trailing blank line).
        let input = "event: session\ndata: {\"id\":\"s2\",\ndata: \"patch\":{}}\n";
        let got = frames(input);
        assert_eq!(
            got,
            vec![FeedEvent::Diff {
                kind: "session".into(),
                id: "s2".into(),
                agent: None,
                patch: json!({}),
            }]
        );
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode_segment("a b/c:d"), "a%20b%2Fc%3Ad");
        assert_eq!(encode_segment("plain-1_ok.~"), "plain-1_ok.~");
    }
}
