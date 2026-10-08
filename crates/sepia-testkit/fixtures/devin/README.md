# Golden fixtures — Devin adapter

Each case dir holds `store.sql` — a text dump of a real `sessions.db` (rebuild
with `rusqlite::Connection::execute_batch`; the dump carries the DDL) — plus the
adapter's `SessionJson` output: `list.json` (repository `list()` result —
summaries only, `nodes`/`promptHistory` empty, `cogsJson` projected to `"[]"`,
sorted by `lastActivityAt` desc) and `export.<id>.json` per listed id (`getById`
with the full node graph). Cases: `basic` (two sepia-written sessions — the
writer's own output shape — covering a minimal user/assistant pair, prompt
history with an `is_shell` row, `metadata` NULL and populated, `hidden: 1`,
non-default `agentMode`/`workspaceDirs`, and list ordering by
`last_activity_at`); `full` (a rich written session — user message blocks via
`metadata.extensions["chisel/acp-content-blocks"]` (text + base64 image +
`resource_link` file), rendered assistant node with signed thinking
(`thinking.signature` → `thinking`/`thinkingSignature`), per-node
`metrics`/`request_id`/`finish_reason`/`generation_model`, tool calls whose
`locations`/`diffs` ride `chisel/tool_call_content`, tool-result nodes with
`chisel/tool_result_meta` + `chisel/terminal_output` exit code +
`chisel/tool_call_timing`, an unsigned-thinking node whose thinking is dropped
on write, `sepia/checkpoints` checkpoint refs folded into `sessions.metadata`,
and a `subagent_heads` row linking `devin-sub-1` → `parentSessionId`);
`raw-chisel` (hand-written store-native blobs: full chisel extension payloads,
`metrics` with `ttft_ms`/cache tokens, `tool_call_state` rows — one real update
asserting `failed` + `terminal_exit` exit code, one malformed blob that is
skipped — a `subagent_heads` link, an ignored `rendered_commits` table, a
tolerated malformed `chat_message` (`{"content":{"odd":true}}` → `system` role
with stringified content) whose row `metadata` is not JSON at all, and a
node-less second session).

Schema a Rust reader must handle: `sessions(id TEXT PK, working_directory,
backend_type, model, agent_mode, created_at INTEGER, last_activity_at INTEGER,
title, main_chain_id, shell_last_seen_index, cogs_json, workspace_dirs, hidden,
metadata)` — all second-resolution epoch except `prompt_history.timestamp`
(milliseconds); `message_nodes(row_id INTEGER PK AUTOINCREMENT, session_id FK,
node_id, parent_node_id NULL, chat_message TEXT, created_at, metadata TEXT
NULL)` — `chat_message` is a JSON blob `{message_id, role, content, thinking?
{thinking, signature}, tool_calls? [{id,name,arguments,index,kind}], metadata?
{num_tokens, is_user_input, request_id, metrics {input_tokens, output_tokens,
cache_read_tokens, cache_creation_tokens}, finish_reason, extensions,
generation_model, created_at ISO, telemetry {source, operation}}}`; chisel
extensions: `chisel/tool_call_content` maps call id →
`{toolCallId,title,status:completed|failed|in_progress|pending,locations
[{path,line?}],kind,rawInput,content? [{type:"diff",path,oldText?,newText?}]}`,
`chisel/tool_result_meta` `{success,kind}` + `chisel/terminal_output`
`{exit:{exit_code}}` + `chisel/tool_call_timing` `{duration_ms}` on tool nodes,
`chisel/acp-content-blocks` = the ACP `ContentBlock[]` (`text`/`image`/`audio`/
`resource`/`resource_link`). Optional tables: `tool_call_state(session_id,
tool_call_id, tool_call_json, tool_call_update_json)` whose update JSON
`{toolCallId,status,_meta["cognition.ai/terminal_exit"].exit_code}` overrides
the tool node's recorded outcome (malformed rows skipped);
`subagent_heads(session_id, agent_id, chain_node_id, updated_at)` where
`agent_id` is the spawned session's id → `parentSessionId`;
`rendered_commits` is never read; `__drizzle_migrations` is ORM bookkeeping.
Session `checkpoints` ride inside `sessions.metadata["sepia/checkpoints"]` as
`[{ref, createdAt(ms), runCount?, kind?}]`. Tool-call outcomes fold back onto
the call: `tool` node results + `tool_call_state` updates set
`status`/`exitCode`/`durationMs` on the matching `toolCalls[]` entry. Wire JSON
is camelCase; `Option` fields (`parentNodeId`, `toolCallId`, `thinking`, …) are
absent when none; `metadata` may be `null`.
