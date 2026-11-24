# Changelog

## [1.0.0] - 2026-11-24

### Added

- **driver-pavo:** tamias-driver-sepia — pavo view driver over sepia serve (`fe01d66`)
- **driver-pavo:** opt views into ViewSpec.refresh_ms (`b2f4b2b`)
- **sepia:** sessions.commands/sessions.command — generic slash-command surface (`9bcb2ca`)
- **driver-pavo:** session-plane mutations over the shared serve child (`875f6b6`)
- **driver-pavo:** declared actions, node bindings, and menus (`870cefe`)
- release infrastructure — v0.1.0 publish metadata, dist + tag-triggered releases (`79d1746`)
- **sepia-driver-pavo:** cli verb is an engine passthrough — the tamias bins[] shim contract (`704db9b`)
- **serve:** sessions.run — one-call ensure+mode+prompt_wait turn (`69c9ad1`)
- **driver-pavo:** P3 live event feed — subscribe child, live views, platform feeds (`487bd1e`)
- **driver-pavo:** P2 view coverage — config, stores, drivers, session meta form (`3c5f4ad`)
- **driver-pavo:** pin pavo deps to interaction-protocol merge + serve invoke op (`307f41b`)
- **driver-sepia:** sessions.* ops queries for the session plane (`221c200`)
- sessions.prompt_wait — blocking turn primitive on the serve wire (`0afc54e`)
- CLI-as-generated-surface — sepia-cli engine lib + tamias-driver-sepia ops (P-021) (`0378153`)

### Changed

- **BREAKING** **meta:** shrink sepia-meta to sessions-only (P-006 phase 3) (`bf93253`)
- **sepia-driver-pavo:** adopt pavo-driver-kit for serve loop and manifest/view construction (`76e885c`)
