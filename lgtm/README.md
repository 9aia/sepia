# sepia observability (LGTM)

`grafana/otel-lgtm` all-in-one: Grafana + Loki (logs) + Tempo (traces) +
Mimir/Prometheus (metrics) + an OTel Collector.

## Run

```bash
docker compose -f lgtm/docker-compose.yml up -d
# or from the root compose, which includes this file:
docker compose up -d lgtm
```

- Grafana: http://localhost:3100 (`admin` / `admin`, or `GF_SECURITY_ADMIN_PASSWORD`)
- OTLP/HTTP ingest: `http://localhost:4318`
- OTLP/gRPC ingest: `localhost:4317`

## Wiring

`sepia-server` exports spans, `Effect.log*` lines, and `Metric` counters over
OTLP/HTTP via `@effect/opentelemetry` (`apps/server/src/telemetry.ts`).

| Env                           | Default                 |
| ----------------------------- | ----------------------- |
| `SEPIA_OTEL`                  | `1` (`0` disables)      |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318` |
| `OTEL_SERVICE_NAME`           | `sepia-server`          |

When sepia-server runs inside compose, point it at the service name:
`OTEL_EXPORTER_OTLP_ENDPOINT=http://lgtm:4318`.

## What to look at in Grafana

- **Explore → Tempo** — `sepia.control.*` spans per control-plane call
  (`attach`, `prompt`, `create_session`, …) plus `http.<method> <path>`
  request spans. Agent subprocess time shows inside `attach`/`prompt`.
- **Explore → Loki** — `{service_name="sepia-server"}` log lines.
- **Explore → Prometheus** — `sepia_*_total` counters (attaches, prompts,
  created/deleted sessions).

No provisioned dashboards yet — Explore is the starting point.
