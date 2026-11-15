# Decisions

ADR-lite: the load-bearing decisions, each with the alternative it
beat and the consequence it carries.

## Mechanism vs policy

**Decision:** sepia owns everything you can do *to* a session;
peregrin owns *who/when/where*. The seam is `sepia serve` — ndjson
JSON-RPC over stdio — so orchestrators never link session internals.

- *Alternative:* one crate doing both, or orchestrators linking
  `sepia-session` directly.
- *Consequence:* policy evolves fast without touching mechanism;
  mechanism stays boring (stable IR, tolerant parsers). Cost: a
  subprocess hop per session op.

## Drivers are binaries, not libraries

**Decision:** store adapters ship as `sepia-driver-<id>`
executables discovered at runtime (`$SEPIA_DRIVER_DIR`, data dir,
`/usr/local/lib/sepia/drivers`, PATH).

- *Alternative:* in-process adapter crates behind a trait.
- *Consequence:* a crashed or hung adapter kills a subprocess, not
your daemon; adding an agent is dropping a binary in a dir. Cost:
  serialization overhead and a second protocol to version.

## One canonical IR, lossy edges marked

**Decision:** every agent store maps onto one session IR; what a
store can't express is flagged `lossy` in metadata.

- *Alternative:* per-agent native models with translation at the
  edges.
- *Consequence:* one read/write path serves every agent and
  conversion is IR→IR; the audit trail shows exactly what was
  dropped.

## The `sepia` command is a tamias shim

**Decision:** no hand-rolled `sepia` binary — `libraries/sepia`
declares `bins: [{name: sepia, shim: cli, driver: sepia}]` over
`tamias-driver-sepia`, whose ops manifest derives from the clap
tree.

- *Alternative:* a `[[bin]]` in `sepia-cli` plus bespoke install
  logic.
- *Consequence:* one distribution path for every org tool; bounded
  verbs become ops, `serve` stays a surface. Cost: the command
  requires tamias installed.

## Blocking turn as a wire primitive

**Decision:** `sessions.prompt_wait` blocks until terminal state
inside the service, with concurrent dispatch so `cancel`/
`permission` interleave.

- *Alternative:* prompt returns immediately; callers poll events.
- *Consequence:* orchestrators get a synchronous turn boundary
  without reimplementing event correlation; the entity-plane
  `sepia.prompt_wait` op carries the same capability.
