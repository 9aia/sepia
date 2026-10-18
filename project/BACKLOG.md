# Backlog

- Windows + macOS support — driver/config dir resolution is already
  platform-agnostic; remaining work is per-OS testing, service
  adapters, and CI targets.
- `sepia serve` expansion — full method surface + lifecycle (idle
  sweep tuning, multi-client policy hooks), see `crates/sepia-cli/src/serve.rs`.
- crates.io publishing — adopt the bupha release model when the wire
  stabilizes.
- Agent config IR write coverage — more per-agent surfaces as drivers
  grow them.
