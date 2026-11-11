# Getting started

This tutorial takes you from zero to listing and prompting a real
agent session through sepia. You will need Rust (`cargo`) and at least
one supported coding agent with existing sessions — Devin, Cline,
Claude Code, or Cursor.

## Install

The `sepia` command ships as a [tamias](https://github.com/9aia/tamias)
pack — a generated shim that resolves `tamias-driver-sepia` at call
time. With `tamias` installed:

```bash
tamias install 9aia:sepia@0.2.0    # org registry (needs TAMIAS_REGISTRY_TOKEN)
```

Or from a checkout:

```bash
cargo xtask pack                                  # stage the driver payload
cargo xtask install                              # tamias-driver-sepia + store drivers → ~/.local
tamias install --allow-drivers ./libraries/sepia # deploy the `sepia` shim
```

`cargo xtask install` alone gives you the driver binaries (the same
layout an installed release uses, so driver discovery works
identically) — the `sepia` command itself comes from the pack step.

## See your sessions

```bash
sepia list                # the Devin store — the default source
sepia list --from cline   # any other agent's store
```

You'll see a row per session — id, title, and working directory. One IR
over every agent: that is the point of sepia.

## Inspect one

```bash
sepia export <session-id> --from devin | head -c 400
```

`export` prints the canonical session IR as JSON — title, history,
tool calls, checkpoints — regardless of which agent produced it.

## Move it to another agent

```bash
sepia install <session-id> --from devin --to claude
```

That copies the session through the IR into Claude Code's store and
prints the id to resume with; without `--to` it lands in the Cline
store, ready for `cline --id <id>`. Same session, different agent —
see [convert a session](../how-to/convert-a-session.md) for ids,
`--force`, and where each agent keeps the files.

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
- [Convert a session](../how-to/convert-a-session.md) — move a session
  into another agent's store, ids and `--force` included.
- [Driver protocol](../reference/protocol.md) — the wire drivers and
  `sepia serve` share.
- [Write a store driver](../how-to/write-a-driver.md) — add a new agent.
