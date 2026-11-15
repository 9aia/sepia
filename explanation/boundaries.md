# Boundaries

What sepia owns — mechanism — and the neighbors it deliberately
doesn't.

## Owns

- The **canonical session IR** and every store→IR mapping.
- **Store drivers** — per-agent `SessionRepository` adapters over
  the driver wire.
- The **local session plane**: attach, prompt, cancel, permission
  resolution, restore/rewind, lock/liveness probes.
- **Agent configs**: the `AgentConfig` IR, `sepia config` ops, and
  the `type: sepia-config` tamias driver.
- The **`sepia serve` wire** — the embedding contract orchestrators
  spawn.
- Session **conversion** between agents (`sepia convert`) and the
  `sepia-meta` overlay store.

## Doesn't own

| Neighbor | Owns instead |
| --- | --- |
| [peregrin](https://9aia.com/peregrin/) | **Policy** — who may act on a session, when, from where: lock registry, takeover arbitration, multi-node merge/route/queue. Peregrin spawns `sepia serve`; it never links the session plane. |
| Agent CLIs themselves | Devin, Cline, Claude Code, Cursor own their stores and runtimes; sepia reads/writes through drivers and ACP. |
| [tamias](https://9aia.com/tamias/) | Distribution — the `sepia` command is a pack `bins[]` shim; tamias owns fetch/install/service lifecycle. |
| [pavo](https://9aia.com/pavo/) | Surfaces — `tamias-driver-sepia` renders read-only views proxying `sepia serve`; the wire stays the contract. |
| [hypha](https://github.com/9aia/hypha) / [peregrin entity plane](https://9aia.com/peregrin/) | Sessions appear there as `kind="sepia-session"` entities via the `sepia-peregrin` adapter — the entity substrate itself is hypha's. |

The rule of thumb: if a feature needs a daemon, a multi-machine
view, or arbitration between actors, it belongs to peregrin. If it
needs a stable wire other repos embed against, it belongs here.
