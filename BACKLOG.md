# Backlog

- T-003 Windows + macOS support — driver/config dir resolution is already platform-agnostic; remaining work is per-OS testing, service adapters, and CI targets.
- T-005 sessions.unsubscribe + subscription dedup — repeated sessions.subscribe on one session spawns duplicate forwarders (correct but redundant); per-client bookkeeping is peregrin's call today. Revisit if the redundancy ever costs.
- T-006 Meta run-spans on import — imports don't record provenance spans (attach does); decide whether a transferred copy should trace its origin.
- T-007 presentation deck — pending: run via ~/.9aia-pres pipeline (pres kick sepia or supervisor); agent writes presentation/ on branch presentation/initial, slidev build verify, push; no PR (sturn flow) — the pushed branch is the deliverable; merge is a human/sturn call; status board: ~/.9aia-pres/STATUS.md · created:2026-11-02
