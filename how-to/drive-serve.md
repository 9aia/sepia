# Drive `sepia serve` from an orchestrator

This guide shows how to embed sepia in an orchestrator — spawning the
service, negotiating the handshake, and calling the plane.

## Spawn and handshake

Spawn `sepia serve` with piped stdio, then send `manifest` first:

```json
{"jsonrpc":"2.0","id":1,"method":"manifest","params":{}}
```

If the returned `protocol` differs from what you speak, stop there —
the surface is versioned and the manifest is the contract. Peregrin
ships this as `peregrin-session::SepiaService` (spawn, handshake,
respawn-once); anything else is ~100 lines over `sepia_driver::rpc`
framing if you are in Rust, or plain line-buffered stdio otherwise.

## Call the plane

```json
{"jsonrpc":"2.0","id":2,"method":"sessions.list","params":{"withLocks":true}}
{"jsonrpc":"2.0","id":3,"method":"sessions.prompt","params":{"id":"<session>","parts":[{"type":"text","text":"hello"}]}}
```

Reads are idempotent and safe to retry. `sessions.attach` with
`takeover` is the only preemptive op — who is *allowed* to take over is
your policy decision, not sepia's.

## Live events

`sessions.subscribe` returns immediately and events arrive as
`session.event` notifications:

```json
{"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"<id>","events":[…]}}
```

Keep stdout parsing tolerant — notifications can interleave with
responses to in-flight requests.

## Lifecycle and failure

- Environment vars (`SEPIA_DRIVER_DIR`, `SEPIA_*_DIR`, DB paths) belong
  in the child's environment, set at spawn.
- Send `shutdown` for a clean exit; EOF on stdin also terminates.
- If the child dies mid-call, respawn once and retry — attach state is
  per-process and is lost with it, so re-attach before prompting.

## A complete example

Peregrin's `crates/peregrin-session` is the reference consumer: typed
methods, a `broadcast::Receiver` for notifications, and respawn-on-death
around every call.
