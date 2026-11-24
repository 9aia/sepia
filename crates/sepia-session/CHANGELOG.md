# Changelog

## [1.0.0] - 2026-11-24

### Added

- **http:** sepia-http — session REST surface over SessionState (PL-027) (`d027657`)
- **sepia:** sessions.commands/sessions.command — generic slash-command surface (`9bcb2ca`)
- **acp:** canonical unattended client — blocking facade + client request lane (PL-029) (`8c79033`)
- release infrastructure — v0.1.0 publish metadata, dist + tag-triggered releases (`79d1746`)
- **session:** sessions.set_mode — ACP session/set_mode on the wire (`19dd84a`)
- **session:** per-session permission policy for unattended runs (`16533e3`)
- sessions.prompt_wait — blocking turn primitive on the serve wire (`0afc54e`)

### Changed

- **BREAKING** **meta:** shrink sepia-meta to sessions-only (P-006 phase 3) (`bf93253`)
- v2 — the session abstraction, standalone (`67b0846`)

### Fixed

- **session:** reap idle agent subprocesses — non-pinning watch + sweep self-heal (`bd71ead`)
- **serve:** sessions.delete falls back to the store wire for runtime-less agents (`9257767`)
- **driver-cline:** time/macros feature + rustfmt new test files (`7b9b5de`)
- **session:** AttachResult serializes camelCase like the rest of the wire (`78fb8ae`)
