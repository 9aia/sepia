# Changelog

## [1.0.0] - 2026-11-24

### Added

- **sepia:** sessions.commands/sessions.command — generic slash-command surface (`9bcb2ca`)
- **acp:** canonical unattended client — blocking facade + client request lane (PL-029) (`8c79033`)
- release infrastructure — v0.1.0 publish metadata, dist + tag-triggered releases (`79d1746`)
- **serve:** sessions.run — one-call ensure+mode+prompt_wait turn (`69c9ad1`)
- **session:** sessions.set_mode — ACP session/set_mode on the wire (`19dd84a`)
- **session:** per-session permission policy for unattended runs (`16533e3`)
- sessions.prompt_wait — blocking turn primitive on the serve wire (`0afc54e`)

### Changed

- v2 — the session abstraction, standalone (`67b0846`)

### Fixed

- **acp:** answer agent requests carrying non-u64 JSON-RPC ids (PL-029) (`c6f66e4`)
- **driver-cline:** time/macros feature + rustfmt new test files (`7b9b5de`)
- **acp:** allowlist DEVIN_PERMISSION_MODE for spawned agents (`262cc52`)
- **acp,host:** kill agent/driver children on drop; bound stdin writes (I-011) (`1c58a67`)
