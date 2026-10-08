//! `web_sys::EventSource` glue — compiled only for the `hydrate`
//! (wasm) build. Both node SSE surfaces are bridged through the hub:
//! `GET /api/sessions/{id}/stream` (unnamed `SessionEvent` frames) and
//! `GET /api/events` (named `session`/`meta`/`project`/`heartbeat`).

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

/// An open `EventSource` plus its callbacks — dropping closes it.
pub struct EventStream {
    source: EventSource,
    _callbacks: Vec<Closure<dyn FnMut(MessageEvent)>>,
}

impl EventStream {
    /// `source.close()` — idempotent, safe to call on cleanup.
    pub fn close(&self) {
        self.source.close();
    }
}

impl Drop for EventStream {
    /// Dropping the stream (e.g. via a `StoredValue` going out of
    /// scope with its owner) closes the underlying `EventSource`.
    fn drop(&mut self) {
        self.close();
    }
}

fn open(url: &str) -> Option<EventSource> {
    match EventSource::new(url) {
        Ok(es) => Some(es),
        Err(e) => {
            leptos::logging::warn!("EventSource {url} failed: {e:?}");
            None
        }
    }
}

fn on_message<F>(es: &EventSource, name: Option<&str>, f: F) -> Closure<dyn FnMut(MessageEvent)>
where
    F: FnMut(MessageEvent) + 'static,
{
    let cb = Closure::wrap(Box::new(f) as Box<dyn FnMut(MessageEvent)>);
    match name {
        None => es.set_onmessage(Some(cb.as_ref().unchecked_ref())),
        Some(n) => {
            let _ = es.add_event_listener_with_callback(n, cb.as_ref().unchecked_ref());
        }
    }
    cb
}

/// `/api/sessions/{id}/stream` — `SessionEvent` JSON frames, plus the
/// `lagged` marker (the broadcast ring dropped events → resync).
pub fn session_stream(
    id: &str,
    agent: Option<&str>,
    mut on_event: impl FnMut(sepia_proto::SessionEvent) + 'static,
    mut on_lagged: impl FnMut() + 'static,
) -> Option<EventStream> {
    let url = match agent {
        Some(a) if !a.is_empty() => format!("/api/sessions/{id}/stream?agent={a}"),
        _ => format!("/api/sessions/{id}/stream"),
    };
    let es = open(&url)?;
    let mut callbacks = Vec::new();

    callbacks.push(on_message(&es, None, move |ev: MessageEvent| {
        if let Some(data) = ev.data().as_string() {
            match serde_json::from_str::<sepia_proto::SessionEvent>(&data) {
                Ok(event) => on_event(event),
                Err(e) => leptos::logging::warn!("bad session event: {e}: {data}"),
            }
        }
    }));
    callbacks.push(on_message(&es, Some("lagged"), move |_ev: MessageEvent| {
        on_lagged();
    }));

    Some(EventStream {
        source: es,
        _callbacks: callbacks,
    })
}

/// `/api/events` — the node feed. Any `session`/`meta`/`project` event
/// calls `on_change` (the list refetches); `heartbeat` is ignored.
pub fn node_feed(on_change: impl FnMut() + 'static) -> Option<EventStream> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let es = open("/api/events")?;
    let on_change = Rc::new(RefCell::new(on_change));
    let mut callbacks = Vec::new();
    for kind in ["session", "meta", "project"] {
        let on_change = Rc::clone(&on_change);
        callbacks.push(on_message(&es, Some(kind), move |_: MessageEvent| {
            (on_change.borrow_mut())();
        }));
    }
    Some(EventStream {
        source: es,
        _callbacks: callbacks,
    })
}
