# Getting started

This tutorial takes you from zero to listing and prompting a real
agent session through sepia. You will need Rust (`cargo`) and at least
one supported coding agent with existing sessions — Devin, Cline,
Claude Code, or Cursor.

## Install

```bash
cargo install --path crates/sepia-cli
```

Or, inside a checkout, `cargo xtask install` builds release `sepia` and
the store drivers into `~/.local` — the same layout an installed
release uses, so driver discovery works identically.

## See your sessions

```bash
sepia list
```

You'll see a table of sessions across every installed agent's store —
id, title, working directory, agent, and last activity. One IR over
every agent: that is the point of sepia.

## Inspect one

```bash
sepia store list --agent devin
sepia export <session-id> --agent devin
```

`export` prints the canonical session IR as JSON — title, history,
tool calls, checkpoints — regardless of which agent produced it.

## The service surface

```bash
sepia serve
```

`sepia serve` exposes the whole session plane as ndjson JSON-RPC over
stdio — the surface [peregrin](https://9aia.com/peregrin/) and other
orchestrators spawn and drive. Try it by hand:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"manifest","params":{}}' | sepia serve
```

You should get back a manifest naming the protocol version and the
method list — `sessions.list`, `sessions.attach`, `sessions.prompt`,
and friends.

## Where next

- [Architecture](../explanation/architecture.md) — why mechanism and
  policy are separate, and what belongs where.
- [Driver protocol](../reference/protocol.md) — the wire drivers and
  `sepia serve` share.
- [Write a store driver](../how-to/write-a-driver.md) — add a new agent.
