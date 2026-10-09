//! SSE glue on `leptos_use::use_event_source` — compiled only for the
//! `hydrate` (wasm) build. Both node SSE surfaces are bridged through
//! the hub: `GET /api/sessions/{id}/stream` (unnamed `SessionEvent`
//! frames + a `lagged` named event) and `GET /api/events` (named
//! `session`/`meta`/`project`/`heartbeat`).

use codee::string::FromToStringCodec;
use leptos::prelude::*;
use leptos_use::core::ConnectionReadyState;
use leptos_use::{
    ReconnectLimit, UseEventSourceOnEventReturn, UseEventSourceOptions,
    use_event_source_with_options,
};
use wasm_bindgen::JsCast;

/// An open `EventSource` handle — `Drop` closes it. Pair with
/// `StoredValue::new_local` (sendwrapped `close` is !Send).
pub struct EventStream {
    close: Box<dyn FnOnce()>,
}

impl EventStream {
    pub fn close(self) {}
}

impl Drop for EventStream {
    fn drop(&mut self) {
        // `use_event_source` returns a sendwrapped close — call it
        // from the same (wasm UI) thread that opened the stream.
        let close = std::mem::replace(&mut self.close, Box::new(|| {}));
        close();
    }
}

fn connect(
    url: &str,
    named: &[&str],
    on_event: impl Fn(&web_sys::Event) -> UseEventSourceOnEventReturn + Send + Sync + 'static,
) -> EventStream {
    let ret = use_event_source_with_options::<String, FromToStringCodec>(
        url.to_string(),
        UseEventSourceOptions::default()
            .immediate(true)
            .reconnect_limit(ReconnectLimit::Infinite)
            .named_events(named.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
            .on_event(on_event),
    );
    if matches!(
        ret.ready_state.get_untracked(),
        ConnectionReadyState::Closed
    ) {
        leptos::logging::warn!("EventSource {url} failed to open");
    }
    EventStream {
        close: Box::new(move || (ret.close)()),
    }
}

/// `/api/sessions/{id}/stream` — `SessionEvent` JSON frames, plus the
/// `lagged` marker (the broadcast ring dropped events → resync).
pub fn session_stream(
    id: &str,
    agent: Option<&str>,
    on_event: impl FnMut(sepia_proto::SessionEvent) + Send + Sync + 'static,
    on_lagged: impl FnMut() + Send + Sync + 'static,
) -> EventStream {
    use std::sync::Mutex;
    let on_event = Mutex::new(on_event);
    let on_lagged = Mutex::new(on_lagged);
    let url = match agent {
        Some(a) if !a.is_empty() => format!("/api/sessions/{id}/stream?agent={a}"),
        _ => format!("/api/sessions/{id}/stream"),
    };
    connect(&url, &["lagged"], move |ev: &web_sys::Event| {
        match ev.type_().as_str() {
            "lagged" => {
                if let Ok(mut f) = on_lagged.lock() {
                    f();
                }
            }
            _ => {
                if let Some(me) = ev.dyn_ref::<web_sys::MessageEvent>() {
                    if let Some(data) = me.data().as_string() {
                        match serde_json::from_str::<sepia_proto::SessionEvent>(&data) {
                            Ok(event) => {
                                if let Ok(mut f) = on_event.lock() {
                                    f(event);
                                }
                            }
                            Err(e) => {
                                leptos::logging::warn!("bad session event: {e}: {data}")
                            }
                        }
                    }
                }
            }
        }
        UseEventSourceOnEventReturn::IgnoreProcessingMessage
    })
}

/// `/api/events` — the node feed. Any `session`/`meta`/`project` event
/// calls `on_change` (the list refetches); `heartbeat` is ignored.
pub fn node_feed(on_change: impl FnMut() + Send + Sync + 'static) -> EventStream {
    use std::sync::Mutex;
    let on_change = Mutex::new(on_change);
    connect(
        "/api/events",
        &["session", "meta", "project"],
        move |_ev: &web_sys::Event| {
            if let Ok(mut f) = on_change.lock() {
                f();
            }
            UseEventSourceOnEventReturn::IgnoreProcessingMessage
        },
    )
}
