//! `GET /api/events` — the node-level feed (docs/protocol.md), port of
//! `routes/events.ts`. Each connection drains a bounded subscription;
//! a client that stops draining is terminated rather than wedging the
//! feed. Heartbeats ride the stream as real `heartbeat` events, not
//! comment pings.

use std::convert::Infallible;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;
use serde_json::json;
use tokio::sync::broadcast;

use crate::AppState;
use crate::routes::{not_found, now_ms_f64};

/// `event: <kind>` + `data: <json>` — the TS `frame()` shape.
fn frame(kind: &'static str, payload: &serde_json::Value) -> Event {
    Event::default()
        .event(kind)
        .data(serde_json::to_string(payload).unwrap_or_else(|_| "null".into()))
}

/// `GET /api/events` — same CORS + bearer rules as the session stream
/// (EventSource clients authenticate via `?access_token`).
pub async fn events(
    State(state): State<AppState>,
    method: Method,
    _req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let mut sub = state.feed.subscribe();
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(256);
    let keep_alive = state.keep_alive;
    tokio::spawn(async move {
        let mut heartbeat = (keep_alive > std::time::Duration::ZERO).then(|| {
            // `setInterval` semantics — the first beat lands after one
            // interval, not immediately.
            tokio::time::interval_at(tokio::time::Instant::now() + keep_alive, keep_alive)
        });
        loop {
            tokio::select! {
                event = sub.recv() => match event {
                    Ok(event) => {
                        if tx.try_send(frame(event.kind, &event.payload)).is_err() {
                            return;
                        }
                    }
                    // A lagged subscriber refetches on its side; keep the
                    // stream alive (the feed's queue bound already dropped
                    // the oldest).
                    Err(broadcast::error::RecvError::Lagged(_)) => {},
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = async {
                    match heartbeat.as_mut() {
                        Some(h) => h.tick().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if tx
                        .try_send(frame("heartbeat", &json!({ "ts": now_ms_f64() })))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx))
    });
    let mut res = Sse::new(stream).into_response();
    res.headers_mut().insert(
        axum::http::header::CONNECTION,
        axum::http::HeaderValue::from_static("keep-alive"),
    );
    res
}
