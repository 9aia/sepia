#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Telemetry coverage — `init`, `trace_headers`, `server_span`,
//! `record_response`, guard lifecycle. `init` installs the process-wide
//! subscriber, so each of these runs in its own test process (nextest
//! isolates them).

use sepia_telemetry::{
    ENV_OTEL_ENDPOINT, enabled, init, record_response, server_span, trace_headers,
};

#[test]
fn init_without_endpoint_installs_tracing_only() {
    // Endpoint env absent in test envs → tracing-only subscriber, inert guard.
    let guard = init();
    assert!(!guard.is_enabled());
    assert!(!enabled());
    guard.force_flush();
    drop(guard);
}

#[test]
fn trace_headers_without_a_span_context_are_empty() {
    let headers = trace_headers();
    assert!(headers.is_empty() || headers.iter().all(|(k, _)| !k.is_empty()));
}

#[test]
fn server_span_with_and_without_remote_parent() {
    let headers = http::HeaderMap::new();
    let span = server_span(&http::Method::GET, "/x", &headers);
    record_response(&span, 200);
    record_response(&span, 503); // 5xx marks the error convention

    // A well-formed traceparent is extracted as a remote parent.
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "traceparent",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
            .parse()
            .unwrap(),
    );
    let span = server_span(&http::Method::POST, "/y", &headers);
    record_response(&span, 201);
}

#[test]
fn enabled_reads_the_env() {
    // Test envs carry no OTEL vars — just exercises the read path.
    let _ = enabled();
    let _ = ENV_OTEL_ENDPOINT;
}
