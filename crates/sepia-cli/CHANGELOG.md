# Changelog

## [1.0.0] - 2026-11-24

### Added

- **sepia:** sessions.commands/sessions.command — generic slash-command surface (`9bcb2ca`)
- release infrastructure — v0.1.0 publish metadata, dist + tag-triggered releases (`79d1746`)
- **sepia-driver-pavo:** cli verb is an engine passthrough — the tamias bins[] shim contract (`704db9b`)
- **serve:** sessions.run — one-call ensure+mode+prompt_wait turn (`69c9ad1`)
- **session:** sessions.set_mode — ACP session/set_mode on the wire (`19dd84a`)
- **session:** per-session permission policy for unattended runs (`16533e3`)
- sessions.prompt_wait — blocking turn primitive on the serve wire (`0afc54e`)
- CLI-as-generated-surface — sepia-cli engine lib + tamias-driver-sepia ops (P-021) (`0378153`)

### Changed

- **BREAKING** **meta:** shrink sepia-meta to sessions-only (P-006 phase 3) (`bf93253`)
- v2 — the session abstraction, standalone (`67b0846`)
- replace sepia-telemetry with lampy git dep (P-042/PL-023) (`53651ce`)

### Fixed

- **session:** reap idle agent subprocesses — non-pinning watch + sweep self-heal (`bd71ead`)
- **config:** faithful instructions-rule conversion + phantom-free diffs (`a98281f`)
- **config:** read sepia:rules blocks back into rule items (`a7b994c`)
- **serve:** sessions.delete falls back to the store wire for runtime-less agents (`9257767`)
- **driver-cline:** time/macros feature + rustfmt new test files (`7b9b5de`)
- **store:** --force overwrites an existing devin session (`78dcb26`)
- **cli:** single-pattern match -> if let (clippy) (`4172032`)
