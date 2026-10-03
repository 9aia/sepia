import { Layer } from "effect";
import * as Otlp from "@effect/opentelemetry/Otlp";
import * as FetchHttpClient from "@effect/platform/FetchHttpClient";

/**
 * Combined OTLP layer: Effect spans, `Effect.log*` lines, and `Metric`
 * counters all export over OTLP/HTTP JSON to the collector's :4318 endpoint.
 * Merging this into the server runtime installs the tracer/logger/metrics
 * fiber refs, so every `Effect.withSpan` in the control plane ships telemetry
 * with zero per-call wiring.
 */
export const otelLayer = (options: {
  readonly endpoint: string;
  readonly serviceName: string;
}): Layer.Layer<never, never, never> =>
  Otlp.layerJson({
    baseUrl: options.endpoint,
    resource: { serviceName: options.serviceName },
  }).pipe(Layer.provide(FetchHttpClient.layer));
