# Golden fixtures — Claude Code adapter

Each case dir holds a synthetic `~/.claude` analogue under `store/` plus the
adapter's output as `SessionJson` wire payloads (`sessionToJson` from
`sepia-convert`): `list.json` is the repository `list()` result (summaries,
`nodes` empty, sorted by `lastActivityAt` desc) and `export.<id>.json` the
full `getById(id)` session. Cases: `full-session` (multi-turn with thinking +
`redacted_thinking` signatures, Edit/MultiEdit/Write/Read/Bash tool calls with
locations+diffs, usage incl. flat and nested ephemeral cache tiers,
`file-history-snapshot` checkpoints with an `isSnapshotUpdate` merge, image /
document blocks, `system` init + compact_boundary entries, an `isMeta` prompt
excluded from `promptHistory`, and `progress`/`queue-operation` plumbing that
emits no node but stays in the uuid chain); `subagents` (main file plus the
current `<parent>/subagents/agent-*.jsonl` layout and the legacy
`agent-*.jsonl` project-root sibling, `isSidechain` entries whose `sessionId`
names the parent, `agentId` carried on entries or derived from the `agent-`
filename, plus an inline `isSidechain` root inside the main file);
`degraded` (malformed/truncated/non-object lines, missing uuids, dangling
`parentUuid` chaining to the previous node, orphan `tool_result` blocks, a
uuid-fallback snapshot ref, and a file with no `cwd` that falls back to the
decoded project slug).

Layout a Rust reader must handle: `projects/<slug>/<sessionId>.jsonl` where the
slug is the cwd with every non-alphanumeric byte flattened to `-` (leading
`-` kept — `encodeProjectDir("/work/proj")` = `-work-proj`; decode maps
`-` back to `/` and ensures a leading `/`). Subagent transcripts live at
`<slug>/<parent-id>/subagents/agent-*.jsonl` (current) or as `agent-*.jsonl`
siblings (legacy). `file-history/<sessionId>/` holds backup blobs named by
`snapshot.trackedFileBackups[path].backupFileName` — the IR only keeps the
path→backup map on `metadata.fileHistory`. Entry taxonomy: `summary`
(`summary` text → title, no node), `user`/`assistant` (uuid/parentUuid
chain, `sessionId`, `cwd`, ISO-millis `timestamp` → epoch-seconds floor,
`gitBranch`/`version`/`slug`/`permissionMode` → session meta, flags
`isSidechain`/`isMeta`/`isCompactSummary`/`isVisibleInTranscriptOnly`,
`requestId`/`toolUseResult` on tool-result entries), `system`
(`subtype`/`level`/`compactMetadata`), `file-history-snapshot` (ref =
`messageId` else `uuid`; `snapshot.timestamp` ms-floor → checkpoint
`createdAt` in ms), and `queue-operation`/`progress`/unknown types that
emit no node yet still anchor the uuid chain. `parentUuid` resolves through
non-node entries to the nearest emitted node; explicit `null` starts a new
root; a dangling id falls back to the previous node. `message.content` is a
string or block array: `text`/`image`/`document` → IR blocks,
`tool_use{id,name,input}` → ToolCall (Edit/MultiEdit/Write inputs → diffs,
paths → locations), `tool_result{tool_use_id,content,is_error}` inside a
`user` entry → `tool` node paired to the earlier call, `thinking.signature`
and `redacted_thinking.data` → thinking + `thinkingSignature`, and usage
reads `input_tokens`/`output_tokens`/`cache_read_input_tokens` /
`cache_creation_input_tokens` or `cache_creation.ephemeral_5m/1h_input_tokens`.
Wire JSON is camelCase; `Option` fields (`parentNodeId`, `toolCallId`,
`thinking`, …) are absent when none.
