# Backlog

- T-003 Windows + macOS support — driver/config dir resolution is already platform-agnostic; remaining work is per-OS testing, service adapters, and CI targets.
- T-004 crates.io publishing — adopt the bupha release model when the wire stabilizes.
- T-005 sessions.unsubscribe + subscription dedup — repeated sessions.subscribe on one session spawns duplicate forwarders (correct but redundant); per-client bookkeeping is peregrin's call today. Revisit if the redundancy ever costs.
- T-006 Meta run-spans on import — imports don't record provenance spans (attach does); decide whether a transferred copy should trace its origin.
