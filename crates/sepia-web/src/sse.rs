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
/// `StoredValue::new_local` (sendwrapped `close` is !Send). The
/// `ready_state` signal tracks connect/open/closed-reconnecting for
/// the UI's "Reconnecting…" affordance.
type CloseSlot = std::sync::Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;

pub struct EventStream {
    close: CloseSlot,
    ready_state: Signal<ConnectionReadyState>,
}

impl EventStream {
    pub fn close(self) {}

    /// `leptos_use`'s live connection state — `Connecting`/`Open`/
    /// `Closing`/`Closed`; `Closed` under `ReconnectLimit::Infinite`
    /// means the backoff loop is retrying.
    pub fn ready_state(&self) -> Signal<ConnectionReadyState> {
        self.ready_state
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        // Fallback close — `on_cleanup` normally wins this first
        // (cleanups run before the arena-removal pass, so `close()`'s
        // StoredValue access is legal there). If the StoredValue drops
        // without a cleanup — inside `Arena::remove`'s write lock —
        // running `close()` now would abort on the nested arena read;
        // defer it by one task instead.
        let Some(close) = self
            .close
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let cb = wasm_bindgen::closure::Closure::once(move || {
            close();
        });
        let _ = leptos::prelude::window().set_timeout_with_callback(cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

/// leptos-use builds its `ready_state`/`message` signals under the
/// owner current at construction — a page owner would die on unmount
/// while `use_event_source`'s forgotten event closures can still fire
/// (events queued before teardown), and `set_ready_state` on a dead
/// signal aborts inside the JS trampoline (`unreachable`). All stream
/// internals live under this root owner instead: they stay writable
/// for the document's lifetime.
fn streams_owner() -> Owner {
    use std::cell::Cell;
    thread_local! {
        static OWNER: Cell<Option<Owner>> = const { Cell::new(None) };
    }
    OWNER.with(|o| {
        let o = o.take().unwrap_or_else(Owner::new);
        let out = o.clone();
        OWNER.with(|c| c.set(Some(o)));
        out
    })
}

fn connect(
    url: &str,
    named: &[&str],
    on_event: impl Fn(&web_sys::Event) -> UseEventSourceOnEventReturn + Send + Sync + 'static,
) -> EventStream {
    // A `message` task can be queued in the browser when the route
    // teardown closes the source — dispatch still runs against it and
    // lands here after the page owner is gone. The caller's callback
    // touches page signals (`refetch`, `.set`), so gate on caller
    // liveness first — `on_cleanup` clears the flag before
    // `StoredValue` drops the stream.
    let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let off = std::sync::Arc::clone(&alive);
    on_cleanup(move || off.store(false, std::sync::atomic::Ordering::Relaxed));
    let ret = streams_owner().with(move || {
        use_event_source_with_options::<String, FromToStringCodec>(
            url.to_string(),
            UseEventSourceOptions::default()
                .immediate(true)
                .reconnect_limit(ReconnectLimit::Infinite)
                .named_events(named.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
                .on_event(move |ev| {
                    if !alive.load(std::sync::atomic::Ordering::Relaxed) {
                        return UseEventSourceOnEventReturn::IgnoreProcessingMessage;
                    }
                    on_event(ev)
                }),
        )
    });
    if matches!(
        ret.ready_state.get_untracked(),
        ConnectionReadyState::Closed
    ) {
        leptos::logging::warn!("EventSource {url} failed to open");
    }
    let close: CloseSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    *close
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(move || (ret.close)()));
    // Eager close on the caller's owner — `on_cleanup` runs before the
    // arena-removal pass, so `close()`'s internal StoredValue writes
    // land while the arena lock is free.
    let early = std::sync::Arc::clone(&close);
    on_cleanup(move || {
        if let Some(c) = early
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            c();
        }
    });
    EventStream {
        close,
        ready_state: ret.ready_state,
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
                                leptos::logging::warn!("bad session event: {e}: {data}");
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
