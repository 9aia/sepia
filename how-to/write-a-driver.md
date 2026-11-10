# Write a store driver

This guide shows how to add a new agent's session store to sepia.

## The shape

A driver is a standalone binary named `sepia-driver-<id>` that speaks
ndjson JSON-RPC over stdio — the same framing `sepia serve` uses. It
answers a `driver.manifest` handshake and the store ops it supports
(`session.list`/`get`/`summary`/`history`, and optionally
`session.save`/`rename`/`delete`, `session.rewind`, `config.read`/
`config.write` — see [the driver protocol](../reference/protocol.md)).

## Steps

1. **Model the store.** Read the agent's on-disk format and map it onto
   the session IR (`sepia_core::Session`). Mark unrepresentable fields
   `lossy` in metadata rather than dropping them.

2. **Implement the ops.** New drivers typically start from
   `crates/sepia-driver-cline` — it is the smallest complete example:
   `store.rs` (the repository), `cline.rs` (format mapping),
   `bin/sepia-driver-cline.rs` (the serve entry point + manifest).

3. **Declare a manifest.** Name, version, protocol level, and the
   capability flags sepia uses to decide what a store supports
   (`sessionStore`, `sessionWrite`, `checkpoints`, `rewind`,
   `agentConfig` — see [the driver protocol](../reference/protocol.md)).

4. **Pass conformance.** Point `sepia-testkit`'s contract suite at your
   driver — it exercises the invariants every store must satisfy
   (round-trip listing, unknown-id behaviour, golden fixtures).

5. **Install.** Drop the binary where discovery finds it:
   `$SEPIA_DRIVER_DIR` → the platform data dir
   (`$XDG_DATA_HOME/sepia/drivers` or `~/.local/share/sepia/drivers`)
   → `/usr/local/lib/sepia/drivers` → anywhere on `PATH`.
   `sepia driver list` shows what the plane can see (`--json` prints
   the probed manifests).

## Conventions

- Blocking I/O (SQLite, file scans) runs on `spawn_blocking`.
- One driver instance per store root — take the dir as an argument or
  `<ID>_DIR` env var (`SEPIA_CLINE_DIR` style).
- Errors are `RpcError` with a `code`/`cause` payload — see
  [the protocol reference](../reference/protocol.md).
