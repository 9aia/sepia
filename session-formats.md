# Session formats — Devin CLI, Cline, Cursor, Claude Code

Reference for the on-disk session stores sepia reads or may read, what the
current IR (`packages/sepia/src/Domain.ts`) normalizes, and a gap analysis for
IR v2. Verified against real stores on this machine (Oct 2026) plus published
schema references for Claude Code (no local data).

## What the IR captures today

`Session` (`Domain.ts:40`): `id, title, workingDirectory, backendType,
agentMode, model, createdAt, lastActivityAt, mainChainId, shellLastSeenIndex,
cogsJson, workspaceDirs, hidden, metadata, nodes[], promptHistory[]`.

`MessageNode` (`Domain.ts:19`): `nodeId, parentNodeId (tree, not a flat list),
role ∈ {system,user,assistant,tool}, content (string), toolCalls[]
{id,name,arguments,index,kind,status,exitCode,durationMs,locations,diffs},
toolCallId, toolName, thinking,
thinkingSignature (opaque provider seal — see gap #4), createdAt
(epoch **seconds**), metadata`. `Session` additionally carries
`checkpoints[]` (workspace-snapshot refs — see the Cline section).

Notable: nodes form a **forest** (node_id + parent_node_id), matching Devin's
branching model. `content` is a flat string — no content-block array, no
attachments. `ToolCall` has no result inline — results are separate
`role:"tool"` nodes paired via `toolCallId`; the call's file footprint rides
on the call itself: `locations[]` (`{path, line?}`, ACP `locations`) and
`diffs[]` (`{path, oldText?, newText?}`, the before/after payloads the store
recorded — ACP `diff` content on Devin, `editor` inputs on Cline).

## Devin CLI

**Store**: `~/.local/share/devin/cli/sessions.db` (SQLite, WAL).
Sepia reads it via `SqliteStorage.ts` / `DbSchema.ts` and is the IR's native
schema — the three tables sepia writes are `sessions`, `message_nodes`,
`prompt_history` (`Storage.ts:26`).

### Tables (real schema, superset of `DbSchema.ts`)

- `sessions(id, working_directory, backend_type, model, agent_mode,
created_at, last_activity_at, title, main_chain_id, shell_last_seen_index,
cogs_json, workspace_dirs, hidden, metadata)` — `metadata` carries
  `total_credit_cost`, `total_acu_cost`, `response_dimensions` (cumulative
  token/message metrics for the UI).
- `message_nodes(row_id, session_id, node_id, parent_node_id, chat_message,
created_at, metadata)` — `chat_message` is a JSON blob (below); `metadata`
  is row-level: `{summarized_from, num_tokens_preceding, is_system_prefix}`.
- `prompt_history(id, content, timestamp, session_id, is_shell)` — pure
  input-recall log (devin reads it `ORDER BY timestamp, rowid`); no node
  key, and `timestamp`/`content` don't join `message_nodes` reliably, so
  a rewind can't truncate it (see `truncateSessionNodes`).
- **`tool_call_state(session_id, tool_call_id, tool_call_json,
tool_call_update_json)`** — serialised **ACP** `ToolCall` /
  `ToolCallUpdate` objects (`title`, `kind`, `status`, `locations`,
  `rawInput`, `_meta.cognition.ai/inferenceToolName`). The update rows are
  sepia's authoritative tool-call status/exit-code source
  (`toolCallStateOutcomes`); the call rows' `locations`/`content` are also
  present in the message blob's `chisel/tool_call_content`, which is where
  the IR reads them — no join needed.
- **`subagent_heads(session_id, agent_id, chain_node_id, updated_at)`** —
  marks which chain nodes spawn sub-agents (empty in this store but the
  schema exists).
- `rendered_commits(session_id, sequence_number, rendered_html, created_at)` —
  server-rendered HTML snapshots (0 rows here). `sequence_number` has no
  counterpart column in `message_nodes` (the V5 forest migration dropped
  the old `messages` table that owned sequence numbers) and the CLI never
  writes the table — the writer is server-side — so a rewind can't map it
  to removed nodes (see `truncateSessionNodes`).
- `app_state(key, value)`.

### `chat_message` blob shape

```jsonc
{
  "message_id": "uuid",
  "role": "system|user|assistant|tool",
  "content": "string",
  // assistant only:
  "tool_calls": [{"id":"call_…","name":"exec","arguments":{…},"index":0,"kind":"function"}],
  "thinking": {"thinking": "…", "signature": "…"},   // signature required for replay
  // tool only:
  "tool_call_id": "call_…",
  "metadata": {
    "num_tokens": 155,
    "request_id": "uuid",
    "finish_reason": "tool_calls|stop",
    "generation_model": "deepseek-v4-1-flash-high",
    "metrics": {"ttft_ms":843,"total_time_ms":1129,"input_tokens":4727,
                "output_tokens":155,"cache_read_tokens":13312,
                "cache_creation_tokens":null,"tpot_ms":1.84,"tokens_per_sec":541},
    "response_dimensions": [/* cumulative UI metrics */],
    "extensions": {
      "chisel/tool_call_content":   {"<callId>": {title,kind,status,locations,
                                     rawInput,content:[{type:"diff",path,
                                     oldText?,newText?}]}},
                                     // locations:[{path,line?}]; content also
                                     // carries non-diff entries (terminal,
                                     // text) — the IR keeps only the diffs
      "chisel/tool_result_meta":    {"success": true, "kind": "execute"},
      "chisel/tool_call_timing":    {"started_at","finished_at","duration_ms"},
      "chisel/terminal_output":     {"text","cwd","exit":{"terminal_id","exit_code"}},
      "chisel/client-message-id":   "…",              // user msgs
      "affogato/cog-context":       {"key": "…"}      // system context provenance
    },
    "telemetry": {"source":"…","operation":"…"},
    "created_at": "ISO-8601"
  }
}
```

**What sepia keeps**: role, content, tool_calls, thinking text **and
`thinking.signature`** (`parseChatMessage` → `thinking`/`thinkingSignature`;
`buildChatMessage` writes the sealed object back and drops unsigned thinking),
tool_call_id,
`tool_result_meta.kind` → toolName, row `metadata` blob. **Dropped on
read** (and rewritten as `null` on save, `Devin.ts:91-96`): `num_tokens`,
`request_id`, `metrics` (all token/timing data), `finish_reason`,
`generation_model`, `response_dimensions`, `extensions` other than
`tool_result_meta`, `telemetry`, `is_user_input`.

### Files outside the DB

- `transcripts/<session-id>.json` — **ATIF-v1.7** export (Agent Trajectory
  Interchange Format): `{schema_version, agent{name,version,model_name,
tool_definitions}, steps[{step_id,timestamp,source,message,model_name,
reasoning_content,tool_calls[{tool_call_id,function_name,arguments}],
observation{results}}], final_metrics{total_prompt_tokens,
total_completion_tokens,total_cached_tokens,total_steps}}`. A second,
  richer transcript of the same session — includes reasoning text.
- `summaries/history_<hex>.md` — generated session summaries.
- `session_locks/<id>.lock` — live-process locks.

**Resumable via**: `devin acp` (`loadSession`) reading `sessions.db` +
`cogs_json` scaffolding (sepia grafts `core/model` cog from donors when
importing, `Conversion.ts:63-78`).

## Cline (CLI)

**Store**: `~/.cline/data/`. Sessions are directories under
`~/.cline/data/sessions/<session-id>/` plus an index DB at
`~/.cline/data/db/sessions.db`. Session ids are `<epoch-ms>_<5 alnum>`
(`Cline.ts:738`). Sepia reads via `Cline.fromDirectory` / `ClineRepository`
and installs via `ClineStore` + `ClineIndex.ts`.

### Per-session directory

- `<id>.json` — **manifest**: `{version, session_id, source:"cli", pid,
started_at, ended_at (ISO), exit_code, status
(running|idle|pending|completed|failed), interactive, provider, model,
cwd, workspace_root, team_name, enable_tools, enable_spawn, enable_teams,
prompt, metadata{title, systemPrompt, mode, sessionHistoryOrigin},
messages_path}`. Sepia's `sessionManifest` (`Cline.ts:750`) reproduces
  this.
- `<id>.messages.json` — **transcript**: `{version, updated_at, agent,
sessionId, origin{source,mode,sessionId,version}, system_prompt,
messages[]}`.

### `messages[]` entry shape

```jsonc
{ "id": "msg_…", "role": "user|assistant", "ts": <epoch ms>,
  // assistant only:
  "modelInfo": {"id":"deepseek/deepseek-v4-flash","provider":"cline-pass"},
  "metrics": {"inputTokens":5720,"outputTokens":279,
              "cacheReadTokens":0,"cacheWriteTokens":0 /*, "cost" */},
  "content": [
    {"type":"text","text":"…"},
    {"type":"thinking","thinking":"…"},
    {"type":"tool_use","id":"call_…","name":"run_commands",
     "input":{"commands":[…]}},
  ]}
// tool results arrive as role:"user" messages containing:
{ "type":"tool_result","tool_use_id":"call_…","name":"run_commands",
  "content":[{"query":"…","result":"…","success":true}] }  // per-item entries
```

Tool vocabulary observed: `read_files{files|paths|path}`, `run_commands
{commands|command}`, `search_codebase{queries|query}`, `fetch_web_content
{requests|url}`, `editor{path,old_text,new_text}` — sepia maps these to/from
`read|exec|grep|webfetch|edit|write` (`Cline.ts:87-175`, `toClineToolName`).
One logical call can fan out to N calls and its result answers per item —
sepia's `toolResultValues` re-pairs them by `query` key.

### Compaction

`<id>.compaction.json` — `{conversation_id, source_message_count,
source_prefix_hash, source_last_message_key, messages[]}` — a rewritten
prefix with a `<SYSTEM_NOTICE>` summary injected. Orphaned `tool_result`s
(call dropped by compaction) are kept as plain user text on import
(`Cline.ts:493-506`).

### Index + sub-agents

`db/sessions.db`:

- `sessions` — columns in `ClineIndex.SESSION_COLUMNS` (`ClineIndex.ts:11`),
  including **`parent_session_id`, `parent_agent_id`, `agent_id`,
  `conversation_id`, `is_subagent`, `team_name`** — a real parent/child tree.
- `subagent_spawn_queue(root_session_id, parent_agent_id, task,
system_prompt, created_at, consumed_at)` — queued sub-agent spawns.

Sub-agent sessions have ids `<parent>__teamtask__<task>__<rand>` with their
own manifest dir, **but their `messages_path` points inside the parent's
dir** (`sessions/<parent>/<task>__<rand>.messages.json`). `ClineRepository`
currently lists each `__teamtask__` dir as a flat sibling session — the
parent/child link is in the index row but not in the manifest the repo
reads.

### Checkpoints

The manifest's `metadata.checkpoint` blob is the checkpoint log:

```jsonc
"checkpoint": {
  "latest":  {"ref":"c4bf…","createdAt":1789096199701,"runCount":54,"kind":"stash"},
  "history": [{"ref":"a325…","createdAt":1789005406769,"runCount":1,"kind":"stash"}, …]
}
```

`ref` is a sha in the workspace's shadow git (`kind` ∈ `stash|commit` —
stash carries uncommitted state, commit a real checkpoint commit);
`createdAt` is epoch ms; `runCount` the agent run it was taken before.
`metadata.checkpointEnabled` is the on/off flag. `latest` normally repeats
the history tail. `checkpoint-scratch/<hash>/` holds real git
`index`/`pathspec` files — the scratch dirs the shadow-repo bookkeeping
stages from. The IR keeps these as `Session.checkpoints[]` (refs only, never
payloads) and `sessionManifest` writes `{latest, history}` back verbatim.

**Restore**: the refs resolve in the workspace's own git object store — stash
kind is a synthetic 3-parent commit (base, index, untracked) with message
`cline checkpoint session=<id> run=<n>`; commit kind points at a real commit.
`POST /api/sessions/:id/restore {checkpoint}` materializes the covered file
set (`git diff --name-only <ref>^ <ref>`) via `git show <ref>:<path>`; the
scratch dirs are staging leftovers and aren't needed for restore.

**Resumable via**: `cline --id <session-id>`; requires the index row +
manifest + messages trio sepia's `installCline` writes.

## Cursor

Two distinct stores on this machine — a **format surprise**: Cursor keeps
chat data per-workspace/per-chat in the new agent CLI, but all IDE chats in
one **global** KV store; neither is per-session-file like Claude Code.

### A. Agent CLI (`cursor agent`) — `~/.cursor/`

- `chats/<workspace-hash>/<chat-uuid>/` per chat (the hash is opaque — not
  a decodable slug):
  - `meta.json` — `{schemaVersion, createdAtMs, updatedAtMs, title,
hasConversation, cwd?}` (optional file; `cwd` only on some chats).
  - `prompt_history.json` — plain JSON array of submitted prompt strings.
  - `store.db` (SQLite): `blobs(id TEXT PK, data BLOB)` +
    `meta(key,value)`.
    - `meta['0']` is **hex-encoded JSON**: `{agentId, latestRootBlobId,
name, mode, isRunEverything, createdAt, lastUsedModel}`.
    - `blobs` is a **content-addressed DAG** (id = SHA-256). The
      `latestRootBlobId` names a protobuf-ish **checkpoint** blob; its
      repeated field-1 entries are the 32-byte ids of the **ordered message
      list** at that checkpoint, field 9 is the workspace `file://` URI,
      field 22 the client tag (`"cli"`). Checkpoints chain incrementally —
      each new one repeats the full message list, so the latest root is the
      whole transcript. Prompt records (field 1 = prompt text, field 2 =
      uuid, field 10 = parent checkpoint) and UI/tool projections (rendered
      markdown, turn titles, tool-call displays, token counters) are the
      other binary blobs; sepia decodes only the checkpoint message refs.
    - Message blobs are AI-SDK-style JSON:
      - `{"role":"system","content":"…system prompt…"}`
      - `{"role":"user","content":"<user_info>…env context…</user_info>"}` —
        environment context, not a real prompt
      - `{"role":"user","content":[{"type":"text","text":"<user_query>…"}],
"providerOptions":{"cursor":{"requestId":"…"}}}`
      - `{"role":"assistant","content":[{"type":"redacted-reasoning",
"data":"<opaque>"},{"type":"text",…},{"type":"tool-call",
"toolCallId":"tool_…","toolName":"Shell","args":{…}}],"id":"1"}`
      - `{"role":"tool","content":[{"type":"tool-result","toolCallId":"…",
"toolName":"Shell","result":"Exit code: 0\n\nCommand output:…"}],
"providerOptions":{"cursor":{"highLevelToolCallResult":{"output":
{"success":{…,"executionTime":ms},"isError":false}}}}}`
    - **Thinking is `redacted-reasoning` — opaque/encrypted**, not
      recoverable as text from this store; sepia records a `[redacted]`
      thinking marker and keeps the blob verbatim as `thinkingSignature`.
    - No per-message timestamps or usage — `meta.json`/`meta['0']` carry
      session-level `createdAtMs`/`updatedAtMs` only.
- `projects/<project-slug>/agent-transcripts/<chat-id>/<chat-id>.jsonl` —
  a **lossy projection** kept even after `store.db` is pruned (357 files /
  ~11k lines here vs. 5 store.dbs): one JSON line per message
  (`{"role":"user|assistant","message":{"content":[…]}}`) with `text` and
  `tool_use{name,input}` blocks — `[REDACTED]` marks redacted reasoning —
  plus terminal events `{"type":"turn_ended","status":"error","error":…}`.
  `subagents/<uuid>.jsonl` files are sub-agent transcripts (parent = dir
  name). No tool results, no usage, no timestamps — the file mtime is the
  only clock. Chat ids are shared with `chats/`; the store wins when both
  exist (sepia falls back to the transcript when the store is pruned).
- `plans/*.plan.md` — generated plan docs; `agent-cli-state.json`,
  `cli-config.json` — CLI state.

### B. IDE chat — `~/.config/Cursor/User/globalStorage/state.vscdb`

Single global SQLite (`cursorDiskKV` KV table; ~1747 sessions / ~126k
bubbles on this machine):

- `composerData:<composerId>` — chat header: `name` (title),
  `fullConversationHeadersOnly` (ordered `[{bubbleId,type}]`, type 1=user,
  2=assistant), `unifiedMode` (agent/ask/edit), `forceMode`,
  `modelConfig{modelName,maxMode}`, `createdAt/lastUpdatedAt` (ms),
  `contextTokensUsed/contextTokenLimit/contextUsagePercent`, `status`,
  `todos`, `richText` (Lexical), `subComposerIds/subagentComposerIds`
  (sub-agent links), `branches`, `capabilities`, `totalLinesAdded/Removed`,
  `codeBlockData`, `originalFileStates` (pre-edit file contents).
- `bubbleId:<composerId>:<bubbleId>` — message bubble, ~80 fields: `type`
  (1=user/2=assistant), `text`, `richText`, `thinking{text}`,
  `allThinkingBlocks`, `toolFormerData{tool(enum),toolCallId,status,
params/rawArgs}`, `suggestedCodeBlocks`, `assistantSuggestedDiffs`,
  `diffHistories`, `fileDiffTrajectories` (**file diffs**),
  `tokenCount{inputTokens,outputTokens}`, `timingInfo{clientRpcSendTime,
clientSettleTime}`, `usageUuid`, `requestId`, `serverBubbleId`,
  `modelInfo{modelName}`, `images`, `attachedFileCodeChunksMetadataOnly`,
  `webCitations`, `aiWebSearchResults`, `relevantFiles`, `humanChanges`,
  `supportedTools`, `todos`, `consoleLogs`, `multiFileLinterErrors`.
  Serialized values are Python-literal-ish (`'…'`, `True/False`) inside
  JSON — needs a tolerant parser.

**Resumable via**: Cursor agent CLI can resume chats by id; the IDE store
is UI-oriented (bubbles/diffs) rather than a clean provider transcript —
the agent-CLI `store.db` messages are the canonical replay form.

## Claude Code

**Store**: `~/.claude/projects/<cwd-encoded>/<session-uuid>.jsonl` — one
append-only JSONL file per session (no local data; schema from published
references, Dec 2025). Sepia reads via `ClaudeCode.fromFile` /
`ClaudeCodeRepository` and writes via `ClaudeCode.toJsonl` /
`ClaudeCodeRepository.save` (canonical `<slug>/<id>.jsonl` layout).
Sub-agent transcripts live in
`<session-uuid>/subagents/agent-<uuid>.jsonl` (current layout — every entry
`isSidechain: true`, `sessionId` = parent's) or as `agent-*.jsonl` siblings
(legacy). Sibling dirs: `todos/<sid>-agent-<sid>.json`,
`file-history/<sid>/<hash>@v<n>` (pre-edit file backups for undo),
`debug/<sid>.txt`, `session-env/<sid>/`.

### Entry types (`type` field)

Common fields: `uuid`, `sessionId`, `parentUuid` (**conversation tree**,
like Devin's node forest), `timestamp` (ISO ms), `cwd`, `gitBranch`,
`version`, `isSidechain` (true for Task/sub-agent branches), `userType`,
`slug`.

- `summary` — `{summary, leafUuid}` — one-line session title for listings.
- `user` — `message.content` is a string _or_ block array: `text`,
  `tool_result{tool_use_id,content,is_error}`, `image{source{
type:base64,media_type,data}}`, document blocks. Sidecar fields:
  `toolUseResult` (structured result incl. Task-agent `status`,
  `agentId`), `isCompactSummary`, `isVisibleInTranscriptOnly`, `isMeta`,
  `thinkingMetadata`, `todos`.
- `assistant` — `message{model:"claude-opus-4-5-…", id:"msg_…",
content[], stop_reason, usage{input_tokens,output_tokens,
cache_creation_input_tokens,cache_read_input_tokens,
cache_creation{ephemeral_5m…,ephemeral_1h…},service_tier}}`, `requestId`.
  Content blocks: `text`, `thinking{thinking,signature?}` /
  `redacted_thinking`, `tool_use{id:"toolu_…",name,input}` — incl.
  `Task{description,prompt,subagent_type}` for sub-agents, `server_tool_use`.
- `system` — `subtype` ∈ `init|stop_hook_summary|local_command|
compact_boundary`; `level`; `compactMetadata{trigger,preTokens}`;
  `logicalParentUuid`.
- `file-history-snapshot` — `{messageId,snapshot{trackedFileBackups{
path:{backupFileName,version,backupTime}}}}` — **checkpoints**.
- `queue-operation` — queued prompts.

**Resumable via**: `claude --resume <session-id>` / `-c`; the JSONL is the
single source of truth (provider transcript with usage per message,
sidechains reconstructable via `parentUuid`+`isSidechain`).

## Gap analysis

Fields each agent persists vs. what the IR normalizes:

| Field                        | Devin                                    | Cline                              | Cursor                               | Claude Code                               | IR today                      |
| ---------------------------- | ---------------------------------------- | ---------------------------------- | ------------------------------------ | ----------------------------------------- | ----------------------------- |
| Message tree (branch/rewind) | ✅ parent_node_id                        | ❌ flat                            | headers list                         | ✅ parentUuid                             | ✅ parentNodeId               |
| Timestamps per message       | ✅ ISO+epoch                             | ✅ ms                              | ✅ ms/ISO                            | ✅ ISO                                    | ✅ createdAt (s)              |
| Token usage per message      | ✅ metrics.*                             | ✅ metrics                         | ✅ tokenCount                        | ✅ usage (incl. cache tiers)              | ❌ dropped                    |
| Cost / credits               | ✅ session metadata (acu/credit)         | ~ cost field                       | ~ usageUuid                          | service_tier only                         | ❌                            |
| Model per message            | ✅ generation_model                      | ✅ modelInfo                       | ✅ modelInfo/lastUsedModel           | ✅ message.model                          | ❌ session only               |
| Request id                   | ✅ request_id                            | ❌                                 | ✅ requestId                         | ✅ requestId                              | ❌                            |
| Tool call args               | ✅ arguments                             | ✅ input                           | ✅ toolFormerData                    | ✅ input                                  | ✅                            |
| Tool call result             | ✅ tool node + terminal_output ext       | ✅ tool_result                     | ✅ tool-result                       | ✅ tool_result(+toolUseResult)            | ✅ tool node                  |
| Tool call status/error       | ✅ tool_call_state + result_meta.success | result.success                     | status field                         | is_error                                  | ❌ assumed success            |
| Tool timing                  | ✅ tool_call_timing                      | ❌                                 | ✅ timingInfo                        | ❌                                        | ❌                            |
| Reasoning/thinking           | ✅ text + **signature**                  | ✅ thinking(+signature)            | ⚠️ redacted-reasoning (opaque)       | ✅ thinking / redacted_thinking           | ✅ text + `thinkingSignature` |
| File diffs / edits           | ✅ tool_call_content content[].diff      | via editor args                    | ✅ suggestedCodeBlocks/diffHistories | via Edit tool args + file-history backups | ✅ ToolCall.diffs             |
| Images/attachments           | ✅ chisel/acp-content-blocks (ACP)       | image/document blocks              | ✅ images, attached chunks           | ✅ image/document blocks                  | ✅ blocks (non-text only)     |
| Sub-agent/task trees         | ⚠️ subagent_heads table                  | ✅ parent_session_id/agent_id/team | ✅ subComposerIds                    | ✅ isSidechain + Task tool                | ❌                            |
| Checkpoints/file history     | ❌                                       | ✅ manifest checkpoint {latest,    | ✅ originalFileStates                | ✅ file-history-snapshot                  | ✅ Session.checkpoints (refs) |
|                              |                                          | history} + scratch git             |                                      |                                           |                               |
| Tool-call file locations     | ✅ tool_call_content locations           | via tool args                      | toolFormerData paths                 | via tool args                             | ✅ ToolCall.locations         |
| Compaction/summaries         | ✅ summarized_from row meta              | ✅ .compaction.json                | ✅ summarizedComposers               | ✅ isCompactSummary+compact_boundary      | ⚠️ raw in node.metadata       |
| Prompt history               | ✅ prompt_history table                  | prompt field/manifest              | ✅ prompt_history.json               | user entries                              | ✅ promptHistory              |
| Shell/exec identity          | ✅ terminal_id, cwd, exit_code           | run_commands items                 | Shell tool                           | Bash tool                                 | ❌ flattened to exec          |
| Session status/lifecycle     | locks + hidden flag                      | ✅ status, exit_code, pid          | status                               | implicit                                  | ❌                            |
| Git context (branch/sha)     | ❌                                       | ❌                                 | branches field                       | ✅ gitBranch                              | ❌                            |
| Web citations                | ❌                                       | fetch tool                         | ✅ webCitations                      | WebSearch result blocks                   | ❌                            |
| Todos/plans                  | ❌                                       | team tasks                         | ✅ todos, plans/*.md                 | ✅ todos files, TodoWrite                 | ❌                            |

### Ranked gaps

1. **Token usage & cost per message** — all four agents store it; IR drops
   it everywhere (`Devin.ts` writes `metrics:null`, Cline import writes
   zeros into `metrics`). Highest value: billing display, model
   comparison, compaction decisions.
2. **Tool call status + structured result envelope** — IR assumes every
   result succeeded (`success:true` hardcoded in `Devin.ts:123`,
   `Cline.ts:700`). Devin's `tool_call_state`/`tool_result_meta`,
   Claude's `is_error`, Cline's `result.success` all carry failure.
   Needed for honest replay and UI.
3. **Sub-agent/session tree** — Cline already persists
   `parent_session_id`/`agent_id`/`is_subagent`; Claude has
   `isSidechain`/`Task`; Devin has `subagent_heads`; Cursor has
   `subComposerIds`. IR has no session-link field, and `ClineRepository`
   flattens `__teamtask__` dirs into siblings.
4. ~~**Thinking signatures / opaque reasoning**~~ — **done**: Devin and
   Claude seal thinking blocks (`signature`, `redacted_thinking`); Cursor
   fully redacts. IR keeps the seal in `MessageNode.thinkingSignature`
   (opaque, verbatim; last one wins when several sealed blocks fold into a
   node) and writers echo it back — Devin writes `thinking{thinking,
signature}`, Cline writes `signature`/`redacted_thinking`. Unsigned
   thinking is still dropped on write: the backend rejects it on replay.
5. **Per-message model + requestId** — mixed-model sessions (sub-agents,
   model switches) can't be represented; `session.model` is single.
6. ~~Attachments / content blocks~~ — **done**: `MessageNode.blocks` holds
   the ordered block list (`text|image|audio|file`) when a store records
   non-text content — Devin's `chisel/acp-content-blocks` (ACP blocks) and
   Cline `image`/`document` entries map in and out; `content` stays the
   text projection.
7. **Lifecycle/status on Session** — Cline's `status`/`exit_code`/`pid`,
   Devin's `hidden`/locks, Claude's implicit completeness. Needed for a
   correct "resumable vs. live vs. failed" listing.
8. ~~**Checkpoints / file diffs**~~ — **done**: the IR records refs, not
   payloads. `Session.checkpoints[]` keeps Cline's manifest
   `metadata.checkpoint` history (`{ref, createdAt, runCount, kind}` shadow
   -git shas, written back verbatim on export; a Devin-DB copy rides in the
   session metadata under `sepia/checkpoints`). `ToolCall.locations[]` /
   `diffs[]` keep ACP `locations` and `diff` content — Devin's real
   `{type:"diff", path, oldText?, newText?}` payloads and Cline's `editor`
   inputs. Cursor `originalFileStates` and Claude `file-history-snapshot`
   remain unread; they'd map onto the same shapes.
9. **Git context** — Claude records `gitBranch` per entry; cheap to add
   to Session.
10. **Tool call timing + exec env** (cwd, terminal_id, exit_code) — nice
    for diagnostics; Devin has it all in extensions, dropped today.

## IR v2 — recommended additions

Concrete, additive changes to `Domain.ts` (all optional/`default`-ed so
existing stores decode unchanged). Sources in parentheses.

```ts
// MessageNode
usage: Option<{
  inputTokens: number; outputTokens: number;
  cacheReadTokens?: number; cacheWriteTokens?: number;
  thinkingTokens?: number; cost?: number;
}>,                              // Devin metrics, Cline metrics, CC usage, Cursor tokenCount
model: Option<string>,           // per-message model (all agents)
requestId: Option<string>,       // Devin request_id, CC requestId, Cursor requestId
finishReason: Option<string>,    // Devin finish_reason, CC stop_reason
thinkingSignature: Option<string>, // ✅ done — thinking-block seal (Devin
                                 // signature, CC signature/redacted_thinking.data,
                                 // Cursor redacted-reasoning.data)

// ToolCall
status: Option<"pending"|"running"|"completed"|"failed"|"cancelled">,
                                 // Devin tool_call_state, CC is_error, Cursor status
result: Option<unknown>,         // or keep tool nodes but add:
isError / exitCode / durationMs  // Devin tool_result_meta+timing+terminal_output
locations: {path:string; line?:number}[], // ✅ done — ACP tool_call locations
diffs: {path:string; oldText?; newText?}[], // ✅ done — ACP diff content, Cline
                                 // editor inputs; file-change before/after
kind: "read"|"edit"|"execute"|"search"|"fetch"|"function"|…,
                                 // ACP kind; Devin tool_call_state.kind, Cursor tool enum

// Content blocks (replace or augment `content: string`)
blocks: Option<ContentBlock[]>   // {type:"text"}|{type:"image",mediaType,data|uri}
                                 // |{type:"file",path}|{type:"thinking",…}
                                 // Claude image/document, Cursor images/attachments

// Session
parentSessionId: Option<string>, // Cline parent_session_id, CC sidechain root,
                                 // Cursor subComposerIds, Devin subagent_heads
agentId: Option<string>,         // Cline agent_id/team_name
status: Option<"running"|"idle"|"completed"|"failed">,
exitCode: Option<number>,        // Cline manifest/index
git: Option<{branch:string; sha?:string}>,   // CC gitBranch (+sha if captured)
checkpoints: {ref:string; createdAt:number; runCount?; kind?}[],
                                 // ✅ done — Cline checkpoint-scratch hash, CC file-history
citations: Option<{title,url}[]> // Cursor webCitations, CC WebSearch results
```

Priority order for implementation mirrors the ranked gaps: `usage` →
tool `status`/`isError` → `parentSessionId` → ~~thinking `signature`~~
(done) → per-message `model`/`requestId` → `blocks`/attachments → `status`/
`exitCode` → `git`/`checkpoints`.

## Format surprises worth remembering

- **Cursor stores chats per-workspace/per-chat, not per-session-file** —
  agent-CLI chats live under `~/.cursor/chats/<ws-hash>/<chat-id>/` as a
  content-addressed `blobs` SQLite store; IDE chats live in **one global**
  `state.vscdb` KV table keyed `composerData:<id>` /
  `bubbleId:<composer>:<bubble>` (~126k bubbles here). The
  `agent-transcripts/*.jsonl` files are a lossy projection (text +
  `tool_use`, no results/usage), not the canonical transcript — but they
  survive `store.db` pruning, so they're the only record of most chats.
- **Cursor reasoning is unrecoverable**: `redacted-reasoning` blobs are
  opaque — only Devin/Cline/Claude carry readable thinking (and only
  Devin/Claude seal it with signatures).
- **Cline sub-agent transcripts live in the parent's session dir**
  (`sessions/<parent>/<task>__<rand>.messages.json`) while their manifests
  live in separate `__teamtask__` dirs; the link exists only in the index
  DB's `parent_session_id`, which `ClineRepository` doesn't read.
- **Devin keeps a second, richer transcript**: `transcripts/*.json` is
  ATIF-v1.7 with `reasoning_content` and `final_metrics` — separate from
  `sessions.db` and currently unused by sepia.
- **Devin writes every imported assistant turn twice** (unrendered +
  rendered twins) — `Cline.visibleNodes` dedups them on export
  (`Cline.ts:703-728`); any new IR importer must do the same.
