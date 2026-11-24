# Changelog

## [1.0.0] - 2026-11-24

### Added

- release infrastructure — v0.1.0 publish metadata, dist + tag-triggered releases (`79d1746`)

### Changed

- v2 — the session abstraction, standalone (`67b0846`)

### Fixed

- **driver-devin:** delete all session-scoped tables, not just three (`e2cce4a`)
- **config:** faithful instructions-rule conversion + phantom-free diffs (`a98281f`)
- **config:** read sepia:rules blocks back into rule items (`a7b994c`)
- **config:** reinstalls stop duplicating instructions into the block (`a56bd91`)
- **driver-cline:** time/macros feature + rustfmt new test files (`7b9b5de`)
- **driver-devin:** save grafts usable cogs — wire imports stay resumable (`312e0bc`)
