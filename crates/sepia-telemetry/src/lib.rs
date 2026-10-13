//! sepia-telemetry — the opt-in OpenTelemetry layer over Sepia's
//! `tracing` stack.
//!
//! [`init`] builds the global subscriber every daemon main installs:
//! the existing `fmt` + `RUST_LOG` (`EnvFilter`) logging, plus — only
//! when an OTLP endpoint is configured — a `tracing-opentelemetry`
//! layer exporting spans to a collector. With no endpoint the
//! subscriber is exactly the tracing-only setup the daemons always
//! used; [`TelemetryGuard`] is inert.
//!
//! Env surface:
//!
//! - `SEPIA_OTEL_ENDPOINT` — OTLP/HTTP base URL
//!   (`http://collector:4318`); `/v1/traces` is appended when absent,
//!   mirroring `OTEL_EXPORTER_OTLP_ENDPOINT` semantics.
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
//!   — the standard vars also enable export (full `OTEL_EXPORTER_*`
//!   config: headers, timeout, compression, protocol).
//! - `SEPIA_OTEL_SERVICE_NAME` — `service.name` resource; falls back to
//!   `OTEL_SERVICE_NAME`, then `"sepia"`.
//! - `OTEL_SERVICE_NAME` / `OTEL_RESOURCE_ATTRIBUTES` — honored by the
//!   SDK's env resource detector.
//!
//! Propagation: `init` installs the W3C `traceparent`/`tracestate`
//! propagator. [`trace_headers`] injects the current span's context for
//! outbound HTTP (hub→node); [`server_span`] builds the inbound
//! `http_request` span with the remote parent attached. Both degrade
//! to no-ops when export is off — injection emits no headers on an
//! invalid context, and extraction yields a plain local-root span.

use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TraceContextExt;
use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer as _};

/// `SEPIA_OTEL_ENDPOINT` — the opt-in switch. OTLP/HTTP base URL.
pub const ENV_OTEL_ENDPOINT: &str = "SEPIA_OTEL_ENDPOINT";
/// `SEPIA_OTEL_SERVICE_NAME` — `service.name` override.
pub const ENV_OTEL_SERVICE_NAME: &str = "SEPIA_OTEL_SERVICE_NAME";
/// Standard endpoint vars the exporter honors — also enable export.
const OTEL_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OTEL_TRACES_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
const OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";

const DEFAULT_SERVICE_NAME: &str = "sepia";
/// OTLP/HTTP signal path appended to base-URL endpoints (mirrors the
/// exporter's own `OTEL_EXPORTER_OTLP_ENDPOINT` handling).
const TRACES_PATH: &str = "/v1/traces";

/// What `init` needs from the environment — kept pure so tests never
/// touch process env (edition 2024 makes `set_var` unsafe).
#[derive(Debug, Clone)]
struct TelemetryConfig {
    /// Fully-resolved `/v1/traces` URL passed to `with_endpoint`, or
    /// `None` to let the exporter read `OTEL_EXPORTER_OTLP_*` itself.
    endpoint: Option<String>,
    service_name: String,
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// `SEPIA_OTEL_ENDPOINT` is a base URL like the standard
/// `OTEL_EXPORTER_OTLP_ENDPOINT` — append the signal path unless the
/// value already names it.
fn traces_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with(TRACES_PATH) {
        base.to_string()
    } else {
        format!("{base}{TRACES_PATH}")
    }
}

/// Parse the telemetry env; `None` means export is off entirely.
fn config_from(get: impl Fn(&str) -> Option<String>) -> Option<TelemetryConfig> {
    let endpoint = nonempty(get(ENV_OTEL_ENDPOINT)).map_or_else(
        || {
            if nonempty(get(OTEL_TRACES_ENDPOINT)).is_some()
                || nonempty(get(OTEL_ENDPOINT)).is_some()
            {
                // Let the exporter's own env resolution pick the URL.
                Some(None)
            } else {
                None
            }
        },
        |base| Some(Some(traces_url(&base))),
    )?;
    let service_name = nonempty(get(ENV_OTEL_SERVICE_NAME))
        .or_else(|| nonempty(get(OTEL_SERVICE_NAME)))
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());
    Some(TelemetryConfig {
        endpoint,
        service_name,
    })
}

/// `true` when an OTLP endpoint is configured — the flag callers use to
/// log "telemetry on" at boot. Reads process env.
pub fn enabled() -> bool {
    config_from(|k| std::env::var(k).ok()).is_some()
}

fn build_provider(
    config: &TelemetryConfig,
) -> Result<SdkTracerProvider, opentelemetry_otlp::ExporterBuildError> {
    use opentelemetry_otlp::WithExportConfig;
    let mut builder = opentelemetry_otlp::SpanExporter::builder().with_http();
    if let Some(endpoint) = &config.endpoint {
        builder = builder.with_endpoint(endpoint.clone());
    }
    let exporter = builder.build()?;
    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build())
}

/// Handle keeping the tracer provider alive; `Drop` shuts the SDK down
/// (batch processor flush + exporter close). Inert when export is off.
#[derive(Debug)]
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    /// `true` when this guard is exporting spans.
    pub fn is_enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// Flush pending spans without consuming the guard (shutdown also
    /// flushes — this is for long-lived processes wanting a mid-run
    /// drain).
    pub fn force_flush(&self) {
        if let Some(provider) = &self.provider {
            let _ = provider.force_flush();
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("sepia-telemetry: shutdown failed: {e:?}");
            }
        }
    }
}

/// Install the global subscriber: `fmt` + `EnvFilter` (`RUST_LOG`,
/// default `info`) — the daemons' long-standing behavior — plus the
/// OTLP span layer when an endpoint is configured. Also installs the
/// W3C trace-context propagator so [`trace_headers`]/[`server_span`]
/// work regardless.
///
/// Keep the returned [`TelemetryGuard`] alive for the process's span
/// export; dropping it flushes and shuts the provider down.
///
/// # Panics
/// If a global subscriber is already installed (same contract as
/// `tracing_subscriber::fmt::init`).
pub fn init() -> TelemetryGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    global::set_text_map_propagator(TraceContextPropagator::new());

    let config = config_from(|k| std::env::var(k).ok());
    let (otel_layer, provider, build_err) = match &config {
        Some(config) => match build_provider(config) {
            Ok(provider) => {
                let tracer = provider.tracer(env!("CARGO_PKG_NAME"));
                (
                    Some(tracing_opentelemetry::layer().with_tracer(tracer)),
                    Some(provider),
                    None,
                )
            }
            Err(e) => (None, None, Some(e)),
        },
        None => (None, None, None),
    };

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter.clone()))
        .with(otel_layer.map(|l| l.with_filter(filter)))
        .init();

    if let Some(e) = build_err {
        tracing::warn!("OTEL endpoint configured but exporter build failed; tracing-only: {e}");
    } else if config.is_some() {
        tracing::info!("OTEL tracing enabled (OTLP/HTTP)");
    }
    TelemetryGuard { provider }
}

/// [`Injector`] over a header list — ureq (and any client accepting
/// name/value pairs) consumes the returned `Vec` one `.header()` at a
/// time.
struct VecInjector<'a>(&'a mut Vec<(String, String)>);

impl Injector for VecInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.push((key.to_string(), value));
    }
}

/// [`Extractor`] over an `http::HeaderMap` — case-insensitive header
/// lookup per the `Extractor` contract.
struct HeaderMapExtractor<'a>(&'a http::HeaderMap);

impl Extractor for HeaderMapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}

/// W3C `traceparent`/`tracestate` headers carrying the *current* span's
/// context — inject into outbound requests (hub→node proxy calls).
/// Empty when the current span has no valid OTEL context (export off or
/// no span entered), so callers can apply unconditionally.
pub fn trace_headers() -> Vec<(String, String)> {
    let cx = tracing::Span::current().context();
    let mut headers = Vec::new();
    global::get_text_map_propagator(|p| p.inject_context(&cx, &mut VecInjector(&mut headers)));
    headers
}

/// The inbound `http_request` server span: `method`/`target` fields plus
/// the remote parent extracted from `traceparent`/`tracestate` (no
/// remote → a local-root span, which is the normal browser path).
/// Record `http.status_code` (and `otel.status_code = "error"` on 5xx)
/// onto it once the response is known.
pub fn server_span(
    method: &http::Method,
    target: &str,
    headers: &http::HeaderMap,
) -> tracing::Span {
    let parent = global::get_text_map_propagator(|p| p.extract(&HeaderMapExtractor(headers)));
    let span = tracing::info_span!(
        "http_request",
        http.request.method = %method,
        url.path = %target,
        otel.kind = "server",
        otel.status_code = tracing::field::Empty,
        http.status_code = tracing::field::Empty,
    );
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent);
    }
    span
}

/// Record the terminal response status onto a [`server_span`] — marks
/// 5xx `ERROR` per the OTLP server-span convention.
pub fn record_response(span: &tracing::Span, status: u16) {
    span.record("http.status_code", status);
    if status >= 500 {
        span.record("otel.status_code", "error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn disabled_without_env() {
        // The no-env case: telemetry is a no-op — no config, so `init`
        // would build a tracing-only subscriber.
        assert!(config_from(lookup(&[])).is_none());
        assert!(config_from(lookup(&[("SEPIA_OTEL_ENDPOINT", "")])).is_none());
        assert!(config_from(lookup(&[("SEPIA_OTEL_ENDPOINT", "   ")])).is_none());
    }

    #[test]
    fn sepia_endpoint_appends_traces_path() {
        let config = config_from(lookup(&[("SEPIA_OTEL_ENDPOINT", "http://localhost:4318/")]))
            .unwrap_or_else(|| panic!("endpoint should enable telemetry"));
        assert_eq!(
            config.endpoint.as_deref(),
            Some("http://localhost:4318/v1/traces")
        );
        assert_eq!(config.service_name, "sepia");
    }

    #[test]
    fn sepia_endpoint_verbatim_when_path_present() {
        let config = config_from(lookup(&[(
            "SEPIA_OTEL_ENDPOINT",
            "http://collector:4318/v1/traces",
        )]))
        .unwrap_or_else(|| panic!("endpoint should enable telemetry"));
        assert_eq!(
            config.endpoint.as_deref(),
            Some("http://collector:4318/v1/traces")
        );
    }

    #[test]
    fn standard_otlp_vars_enable_export() {
        for key in [OTEL_ENDPOINT, OTEL_TRACES_ENDPOINT] {
            let config = config_from(lookup(&[(key, "http://collector:4318")]))
                .unwrap_or_else(|| panic!("{key} should enable telemetry"));
            // Endpoint left to the exporter's own env resolution.
            assert!(config.endpoint.is_none());
        }
    }

    #[test]
    fn service_name_precedence() {
        let vars = [
            ("SEPIA_OTEL_ENDPOINT", "http://localhost:4318"),
            ("SEPIA_OTEL_SERVICE_NAME", "sepia-hub"),
            ("OTEL_SERVICE_NAME", "otel-name"),
        ];
        let config = config_from(lookup(&vars)).unwrap_or_else(|| panic!("enabled"));
        assert_eq!(config.service_name, "sepia-hub");

        let config = config_from(lookup(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318"),
            ("OTEL_SERVICE_NAME", "otel-name"),
        ]))
        .unwrap_or_else(|| panic!("enabled"));
        assert_eq!(config.service_name, "otel-name");
    }

    #[test]
    fn disabled_guard_is_inert() {
        let guard = TelemetryGuard { provider: None };
        assert!(!guard.is_enabled());
        guard.force_flush();
    }
}
