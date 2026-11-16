# Security Policy

Report vulnerabilities privately by emailing
luisfuturist+9aia@gmail.com with "SECURITY" in the subject — do
not open public issues, do not use GitHub's advisory or
private-reporting features, and do not disclose details publicly;
contact us first and we will coordinate disclosure privately.

## Scope

- sepia reads and writes agent session stores on the local machine —
  transcripts carry prompts, code, and tool output; treat
  `sepia export` output and any store it touches as confidential.
- `sepia serve` exposes the session plane (attach/prompt/cancel/
  permission/restore/rewind) to whatever can spawn it — there is no
  auth on the stdio wire; drive it from trusted orchestrators only.
- Store drivers (`sepia-driver-*`) and agent CLIs are external
  binaries discovered on `$SEPIA_DRIVER_DIR` → data dirs →
  `/usr/local/lib/sepia/drivers` → PATH — a hostile binary there
  runs with your privileges; keep driver dirs clean.
- `sepia-config` packs (`type: sepia-config`) write agent config —
  rules, skills, commands, hooks, MCP servers — into per-agent
  config dirs. Review pack contents before install; MCP server
  entries can carry credentials and hooks execute in the agent.

## Notes

We respond to reports as quickly as we can; expect acknowledgement
within a few days.
