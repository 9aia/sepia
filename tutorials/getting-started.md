# Getting started

Install `sepia` and list the sessions your agents have stored:

```bash
cargo install --path crates/sepia-cli
sepia sessions
```

`sepia serve` exposes the session plane as ndjson JSON-RPC over stdio —
the surface peregrin (and any orchestrator) drives.
