# TODO — sepia (v2)

Mechanism only: the session plane, drivers, ACP, the `sepia` CLI +
`serve` wire. Orchestration (lock registry, routing, merged views, the
app) is peregrin's — it spawns `sepia serve` per machine. The pre-v2
hub/node/web work all landed and was split out in `67b08464`; this file
tracked it before the split.

## Open

### Wire surface — what an orchestrator can't reach yet

- [x] ~~**Meta overlay**~~ — done: `ControlPlane` owns the `MetaStore`
      (`ControlPlaneOptions::meta`/`node_name`): summaries carry
      title/pin/archive/projectIds/spans, attach records run spans, a
      stored `model` applies when attach doesn't pass one,
      `sessions.create` leaves a pending meta-only row, delete drops
      the overlay. Wire: `sessions.patch_meta` +
      `projects.list|create|rename|delete`; `manifest` reports
      `node: {id, name}` (minted `node_<hex16>` at
      `$SEPIA_HOME/node.json`; `$SEPIA_NODE`/`$SEPIA_NAME` override).
- [ ] **`config.*` methods** — `sepia config list/diff/…` reads the
      agent-config IR locally (`config_ops.rs`); orchestrators can't
      reach it. Needs a config-capable driver wire call + serve
      methods.
- [ ] **`sessions.export` / `sessions.import` / `sessions.convert`** —
      CLI `export` + `install --to` exist and `Convert` is a driver
      capability, but serve lacks all three. Blocks peregrin-side
      cross-node transfer (was `transfer_session`/`transfer_project`
      server fns in v1).
- [ ] **Serve lifecycle** — idle-sweep tuning + multi-client policy
      hooks, per `crates/sepia-cli/src/serve.rs` and BACKLOG.

### Ops / QA

- [ ] **macOS + Windows verification** — driver/config dir resolution
      is platform-agnostic; remaining work is per-OS testing, service
      adapters, CI targets.
- [ ] **crates.io publishing** — adopt the bupha release model when the
      wire stabilizes (`VERSION` is `0.0.0`).
- [ ] **Agent config IR write coverage** — more per-agent surfaces as
      drivers grow them (BACKLOG).
- [ ] **`docs/reference/protocol.md` is stale** — it documents the v1
      node REST API (`/api/*`, SSE, pairing, push) that moved to
      peregrin. Rewrite as the v2 wire reference or replace with a
      pointer + the parts that still apply (resource keys, identity).
