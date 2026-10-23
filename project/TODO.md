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
- [x] ~~**`config.*` methods**~~ — done: `config.read`/`config.write`
      on the driver wire (`AgentConfigStore` trait +
      `Capability::AgentConfig`, all four drivers) behind
      `config.list`/`config.get`/`config.import` on serve. `dir`
      resolves driver-side (cline = workspace root, `SEPIA_*_DIR`
      envs otherwise). CLI config verbs unchanged.
- [x] ~~**`sessions.export` / `sessions.import` / `sessions.convert`**~~
      — done: export = `{session}` bundle; import writes IR via
      `session.save` over the driver wire (fresh id per store
      convention); convert is the node-local read+save shortcut.
- [x] ~~**Serve lifecycle**~~ — done (mechanism half): `manifest`
      reports resolved `config` (idleTtl/sweep/historyLimit/probeIdle);
      `events.subscribe` is a node-wide `node.event` feed — lifecycle
      markers (attached/created/detached{reason}/deleted), per-session
      event batches, honest `lagged` counts. Also fixed the silent
      lag-death in `sessions.subscribe` forwards. Multi-client
      *arbitration* stays peregrin's by the boundary.

### Ops / QA

- [ ] **macOS + Windows verification** — driver/config dir resolution
      is platform-agnostic; remaining work is per-OS testing, service
      adapters, CI targets.
- [ ] **crates.io publishing** — adopt the bupha release model when the
      wire stabilizes (`VERSION` is `0.0.0`).
- [x] ~~**Agent config IR write coverage**~~ — done: writes preserve
      provenance across all four drivers (memory-file ownership, .mdc
      extension, cline file-form, claude settings.local partition),
      previously-silent field drops are reported, shared helpers in
      sepia-core.
- [x] ~~**`docs/reference/protocol.md` is stale**~~ — done: rewritten
      as the driver-wire reference (framing, manifest + capabilities,
      session.*/file.restore/config.* methods, -320xx error codes,
      discovery).
