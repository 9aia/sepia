# Contributing

Issues and PRs are **disabled** — the project is agent-maintained.
Fork freely under MIT.

## Layout

`crates/` — rust workspace. `sepia-core` is pure domain (session IR,
repository port, planners); `sepia-acp` is the ACP client;
`sepia-driver` is the driver manifest + serve loop; `sepia-host`
discovers/spawns driver binaries; `sepia-driver-*` are the per-agent
store adapters; `sepia-session` is the local session plane;
`sepia-cli` (`sepia`) is the composition root — CLI verbs plus `sepia
serve`, the stdio session-plane service orchestrators drive.

## Boundary

Mechanism lives here; policy lives in
[peregrin](https://github.com/9aia/peregrin). If a feature needs a
daemon, a multi-machine view, or arbitration between actors (lock
registry, takeover *decisions*, merge/route/queue), it does not belong
in sepia.

## Rules of thumb

- New agent store = a new `sepia-driver-*` binary crate, not a core
  change. New session op = a method on the `sepia serve` wire, not a
  crate-link.
- `cargo xtask check` and `cargo xtask test` must stay green.
- Coverage: `cargo xtask coverage` — cargo-llvm-cov HTML report in
  `target/llvm-cov/html`; pass `--fail-under-lines 100` to gate.
- Drivers are binaries discovered at runtime — never link a driver
  into the plane.
- Commits: conventional prefixes (`feat:`/`fix:`/`refactor:`/`docs:`/`chore:`).
- Never commit secrets.

## Docs

Follow Diátaxis: `docs/tutorials/` (learn) · `docs/how-to/` (tasks) ·
`docs/reference/` (lookup) · `docs/explanation/` (why). The domain
references (`session-formats`, `protocol`, `agent-configs`) are the
source of truth for the IR and both wires — keep them current.
