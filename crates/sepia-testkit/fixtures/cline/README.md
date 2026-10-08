# Golden fixtures — Cline adapter

Each case dir holds a synthetic Cline data dir under `store/` (i.e. what
`~/.cline/data` looks like: `store/sessions/<session-id>/<session-id>.json`
manifest + `<session-id>.messages.json` transcript) plus the adapter's
`SessionJson` output: `list.json` (`list()` result — manifest-derived
summaries, `nodes` empty, `backendType: "cline"`, sorted by `lastActivityAt`
desc) and `export.<id>.json` per listed id (`getById` — the full session built
from the transcript). `index.sql` is a text dump of the `db/sessions.db`
session index a real data dir carries — `ClineRepository` never reads it, so it
is informational (rebuild into `store/db/sessions.db` if a driver wants it);
its `messages_path` column uses the literal `{{DATA_DIR}}` where a real row
holds an absolute path. Cases: `multi-turn` (richest read path — `<user_input>`
prompt unwrapping, `thinking` + `signature`, `redacted_thinking` →
`[redacted]` marker, tool_use mapping `read_files`/`run_commands`/`editor`
(edit + create)/`search_codebase`/`fetch_web_content` →
`read`/`exec`/`edit`/`write`/`grep`/`webfetch` calls plus an unknown tool kept
raw, multi-item result arrays keyed by `query`, per-call `success` flags →
result status, camelCase `metrics` incl. `cost`, `modelInfo.id`, and manifest
`metadata.checkpoint` `{latest, history}` → `checkpoints`); `attachments`
(image/document blocks → IR `image`/`file` blocks incl. base64 and url
sources, and an image-only user turn that is dropped entirely);
`subagent` (`<parent>__agent_<name>` and `<parent>__teamtask__<agent>__<rand>`
ids → `parentSessionId`/`agentId`, each with its own transcript);
`malformed` (a manifest-parseable session whose transcript carries non-array
content, a system-role entry, an orphan `tool_result` → `[tool output]` user
node, a tool call with an unreadable list field kept raw, a skipped empty text
block, plus a broken-manifest session dir, a manifest-less dir, and a stray
file — all skipped by `list()`); `installed` (written end-to-end through
`ClineStore.install` — sepia's own writer output: `msg_<n>` ids, paired
tool_use/tool_result blocks, `UNCAPTURED_TOOL_RESULT` placeholders if a call
lacks a result).

Layout a Rust reader must handle: session dirs named by id (`<epoch-ms>_<5
alnum>`); the manifest is `<id>.json`, the transcript `<id>.messages.json` —
files ending `.messages.json` or containing `.compaction.` are never
manifests. Manifest fields: `session_id`, `cwd`, `workspace_root`,
`started_at`/`ended_at` (ISO → epoch s), `status`, `model` (`cline-pass/`
prefix stripped), `prompt`, `metadata.title`, `metadata.checkpoint`, and
`messages_path` — all fixture manifests set `""` (sibling fallback) since real
stores hold non-portable absolute paths. Transcript: `{version, updated_at,
agent, sessionId, origin, messages[]}` where each message is `{id, role, ts
(epoch ms), content:[blocks], modelInfo{id,provider}, metrics{inputTokens,
outputTokens, cacheReadTokens, cacheWriteTokens, cost}}`; `user` messages carry
`text`/`image`/`document`/`tool_result` blocks, `assistant` messages
`text`/`thinking{signature}`/`redacted_thinking{data}`/`tool_use{id,name,
input}` blocks, and results arrive inside a later `user` message paired by
`tool_use_id` (multi-entry `content` arrays align to multi-item calls by
`query` key, falling back to order). The reader synthesizes two system nodes +
the first user message as the head of the chain, emits each assistant message
as an unrendered/rendered twin pair (row `metadata` carries
`clineMessageIndex`), and mints fresh `chatcmpl-tool-<16 hex>` tool-call ids on
every read — `export-normalized.<id>.json` maps those ids to `call-<n>` in
first-seen order, so compare normalized output (the raw `export.<id>.json` is
not byte-reproducible across runs). Wire JSON is camelCase; `Option` fields are
absent when none.
