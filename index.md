---
layout: home

hero:
  name: sepia
  text: Agent sessions, agnostically
  tagline: One canonical IR, store drivers, and an ACP client for every coding agent.
  actions:
    - theme: brand
      text: Get started
      link: /tutorials/getting-started
    - theme: alt
      text: Reference
      link: /reference/protocol

features:
  - title: Canonical session IR
    details: Every agent's session store maps onto one intermediate representation — one model for titles, history, tool calls, and restores.
  - title: Store drivers
    details: Per-agent adapter binaries speak ndjson JSON-RPC over stdio. Add an agent without touching sepia's core.
  - title: Local session management
    details: sepia-session owns the mechanism — attach, prompt, cancel, permissions, restore/rewind, liveness probing. Orchestration policy lives in peregrin.
---

## What it is

Sepia is the agent-session abstraction: one canonical session IR,
per-agent store drivers speaking ndjson JSON-RPC, an ACP client for
every coding agent, and `sepia serve` — the stdio service
orchestrators embed.

## Why

Every coding agent keeps sessions its own way — SQLite, JSONL,
directory layouts. Sepia maps them all onto one IR so listing,
exporting, converting, and driving sessions is agent-agnostic, and
keeps the mechanism stable enough that peregrin can build policy on
top without linking internals.

## Boundary

- **Owns:** session mechanism — the IR, store drivers, attach,
  prompt, restore/rewind, agent configs, `sepia serve`.
- **Doesn't own:** orchestration policy ([peregrin](https://9aia.com/peregrin/)),
  distribution ([tamias](https://9aia.com/tamias/)), surfaces
  ([pavo](https://9aia.com/pavo/)).

## Where to go next

| You want to… | Go to |
| --- | --- |
| List and prompt a real session | [Tutorial](/tutorials/getting-started) |
| Do a task (convert sessions, write a driver, drive `sepia serve`) | [How-to](/how-to/) |
| Understand the design (IR, drivers, mechanism vs policy) | [Explanation](/explanation/) |
| Look up a verb, wire method, or env var | [Reference](/reference/) |
