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
