# Changelog

## [1.0.0] - 2026-11-24

### Added

- release infrastructure — v0.1.0 publish metadata, dist + tag-triggered releases (`79d1746`)

### Changed

- v2 — the session abstraction, standalone (`67b0846`)

### Fixed

- **host:** driver call timeout 30s→120s — cold agent spawn exceeds 30s (`a643464`)
- **driver-cline:** time/macros feature + rustfmt new test files (`7b9b5de`)
- **acp,host:** kill agent/driver children on drop; bound stdin writes (I-011) (`1c58a67`)
