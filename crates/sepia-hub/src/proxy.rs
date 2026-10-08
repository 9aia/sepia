//! Hub → node HTTP bridges.
//!
//! - [`sse_events`] / [`sse_session_stream`] — long-lived SSE forwards.
//!   ureq is a blocking client, so one `std::thread` per open stream
//!   drains the upstream `BufRead` into an mpsc that an axum `Sse`
//!   response drains. `data:`/`event:` frames pass through verbatim;
//!   comment pings are dropped (axum adds its own keep-alive).
//! - [`passthrough`] — the generic `/api/*` forward, run under
//!   `spawn_blocking`. Bounded bodies only.

use std::convert::Infallible;
use std::io::{BufRead, BufReader};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;

use crate::HubState;

/// Cap on a proxied request body — prompts carry attachments but the
/// node caps them well below this.
const MAX_PROXY_BODY: usize = 16 * 1024 * 1024;

fn upstream_url(state: &HubState, path_and_query: &str) -> String {
    format!("{}{}", state.node_url, path_and_query)
}

fn auth_header(state: &HubState, headers: &HeaderMap) -> Option<String> {
    if let Some(token) = &state.node_token {
        return Some(format!("Bearer {token}"));
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// `GET /api/events` — the node feed (`event:`-named frames).
pub async fn sse_events(State(state): State<HubState>, req: Request<Body>) -> Response {
    let path = path_and_query(&req, "/api/events");
    sse_bridge(&state, &upstream_url(&state, &path), req.headers())
}

/// `GET /api/sessions/{id}/stream` — `SessionEvent` frames.
pub async fn sse_session_stream(
    State(state): State<HubState>,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    let path = path_and_query(&req, &format!("/api/sessions/{id}/stream"));
    sse_bridge(&state, &upstream_url(&state, &path), req.headers())
}

fn path_and_query(req: &Request<Body>, path: &str) -> String {
    match req.uri().query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
}

/// Open the upstream SSE connection and copy frames to the client.
fn sse_bridge(state: &HubState, url: &str, headers: &HeaderMap) -> Response {
    let auth = auth_header(state, headers);
    let mut req = state.sse_agent().get(url);
    if let Some(auth) = auth {
        req = req.header(header::AUTHORIZATION.as_str(), auth);
    }
    let upstream = match req.call() {
        Ok(res) => res,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("node unreachable: {e}")).into_response();
        }
    };
    if !upstream.status().is_success() {
        return upstream_error(upstream).into_response();
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(256);
    // Blocking reader — one thread per open stream. The thread exits
    // when the client drops (blocking_send fails) or upstream closes.
    std::thread::spawn(move || pump_sse(upstream, &tx));
    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx))
    });
    let mut res = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping"),
        )
        .into_response();
    res.headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    res
}

/// Surface a non-2xx upstream as-is (status + `{error}` body).
fn upstream_error(mut res: ureq::http::Response<ureq::Body>) -> (StatusCode, String) {
    let status = res.status().as_u16();
    let body = res
        .body_mut()
        .read_to_string()
        .unwrap_or_else(|_| String::new());
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
        body,
    )
}

/// ureq maps transport timeouts to `Error::Timeout` wrapped in an
/// `io::Error` of kind `Other` once it crosses the `Read` boundary.
fn is_timeout(e: &std::io::Error) -> bool {
    if matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        return true;
    }
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .is_some_and(|e| matches!(e, ureq::Error::Timeout(_)))
}

/// Parse `data:`/`event:` line pairs off the blocking upstream reader
/// and forward them until the stream or the client goes away. The
/// agent carries a 30s body-recv timeout — a stall becomes a
/// keep-alive probe: if the client is gone the `blocking_send` fails
/// and the thread ends.
fn pump_sse(res: ureq::http::Response<ureq::Body>, tx: &tokio::sync::mpsc::Sender<Event>) {
    let mut reader = BufReader::new(res.into_body().into_reader());
    let mut event_name: Option<String> = None;
    let mut data = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) if is_timeout(&e) => {
                // Liveness probe — drops the connection (and this
                // thread) the moment the client is gone.
                if tx.blocking_send(Event::default().comment("ping")).is_err() {
                    break;
                }
                continue;
            }
            Err(_) => break,
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            // Frame boundary — emit if we collected data.
            if !data.is_empty() {
                let mut event = Event::default().data(data.trim_end_matches('\n'));
                if let Some(name) = event_name.take() {
                    event = event.event(name);
                }
                if tx.blocking_send(event).is_err() {
                    break;
                }
            }
            data.clear();
            event_name = None;
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            data.push('\n');
        } else if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim().to_string());
        }
        // `:` comments (upstream keep-alives) and unknown fields drop.
    }
}

/// Generic `/{*rest}` under `/api/` — forward method, body, and the
/// auth/content-type headers; return status + body verbatim.
pub async fn passthrough(State(state): State<HubState>, req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = upstream_url(&state, &format!("{path}{query}"));
    let method = req.method().clone();
    let auth = auth_header(&state, req.headers());
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let Ok(body) = axum::body::to_bytes(req.into_body(), MAX_PROXY_BODY).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
    };

    let agent = state.json_agent();
    let result = tokio::task::spawn_blocking(move || {
        let mut builder = ureq::http::Request::builder()
            .method(method.as_str())
            .uri(&url);
        if let Some(auth) = &auth {
            builder = builder.header(header::AUTHORIZATION.as_str(), auth.as_str());
        }
        if let Some(ct) = &content_type {
            builder = builder.header(header::CONTENT_TYPE.as_str(), ct.as_str());
        }
        let request = match builder.body(body.to_vec()) {
            Ok(r) => r,
            Err(e) => return Err(e.to_string()),
        };
        agent.run(request).map_err(|e| e.to_string())
    })
    .await;

    match result {
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_GATEWAY, format!("node unreachable: {e}")).into_response(),
        Ok(Ok(mut res)) => {
            let status =
                StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = res
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = res.body_mut().read_to_vec().unwrap_or_default();
            let mut response = (status, body).into_response();
            if let Some(ct) = ct {
                if let Ok(v) = HeaderValue::from_str(&ct) {
                    response.headers_mut().insert(header::CONTENT_TYPE, v);
                }
            }
            response
        }
    }
}
