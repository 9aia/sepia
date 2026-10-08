# Golden fixtures — Cursor adapter

Each case dir holds a synthetic `~/.cursor` analogue under `store/` plus the
adapter's `SessionJson` output: `list.json` (summaries, `nodes` empty,
sorted by `lastActivityAt` desc) and `export.<id>.json` per listed id.
Cases: `chat-store` (a full `store.db` chat — system + `<user_info>` +
`<user_query>` user blobs, redacted-reasoning + text + `tool-call` assistant
blobs, tool-result blobs with `providerOptions.cursor.highLevelToolCallResult`
success `executionTime` → `durationMs` and an `isError` result, an opaque
binary blob and a dangling checkpoint ref counted as `opaqueBlobs`, hex
`meta['0']`, `meta.json`/`prompt_history.json` sidecars, and a thinner
same-id transcript the store beats in `list()`/`getById()`);
`transcript-projection` (transcript-only chat, a pruned store.db whose
transcript wins `getById` on node count while `list()` still shows the chat
summary, a meta.json-only chat dir, and a `<chat>/subagents/*.jsonl`
transcript carrying `parentSessionId`); `degraded-store` (a plain-JSON —
not hex — `meta['0']` row, a schema-less store.db with no tables, and an
orphan transcript in a second project slug).

Layout: `chats/<workspace-hash>/<chat-id>/` where the hash dir is `md5` of
`path.resolve(cwd)`, holding `store.db` (shipped here as a `store.sql` text
dump — rebuild with `sqlite3 store.db < store.sql` or
`rusqlite::Connection::execute_batch`; blob literals are `X'hex'`), plus
`meta.json` and `prompt_history.json` sidecars. The store schema is exactly
`blobs(id TEXT PRIMARY KEY, data BLOB)` and `meta(key TEXT PRIMARY KEY, value
TEXT)` with `PRAGMA user_version = 1` (real stores additionally run WAL).
`meta['0']` is the hex encoding of a JSON object — plain JSON is tolerated —
with `agentId`, `latestRootBlobId`, `name`, `mode`, `isRunEverything`,
`createdAt` (epoch ms) and `lastUsedModel`. `blobs.id` is the lowercase
sha256 hex of `data`; `latestRootBlobId` names a protobuf-ish checkpoint
blob whose field-1 length-delimited 32-byte entries are the ordered message
blob ids, field 9 the `file://` workspace URI, field 10 a varint flag, and
field 22 the client tag (`"cli"`). Message blobs are AI-SDK JSON:
`{role:"system",content:string}`, `{role:"user",content:string}` (raw
`<user_info>` plumbing → `metadata.context`) or `content:[{type:"text"}]`
with `providerOptions.cursor.requestId` and `<user_query>` unwrapping,
`{role:"assistant",id:"1",content:[{type:"redacted-reasoning",data?},
{type:"text"},{type:"tool-call",toolCallId,toolName,args}]}`, and
`{role:"tool",content:[{type:"tool-result",toolCallId,toolName?,result}]}`.
The second store is the lossy projection
`projects/<slug>/agent-transcripts/<chat-id>/<chat-id>.jsonl` (slug = cwd
flattened to `-` with leading/trailing dashes trimmed — `home-demo-proj`,
decoded by mapping `-`→`/` and prefixing `/`), one
`{role,message:{content}}` JSON message per line with `text` and
`tool_use{name,input}` blocks, `[REDACTED]` text markers → thinking, and
`{"type":"turn_ended","status":"error",error}` lines → `metadata.turnErrors`;
it carries no timestamps, usage or tool results, so every node takes the file
mtime. Tool inputs project to locations/diffs via `StrReplace`/`Edit`/
`Write`/`Delete`/`ApplyPatch` (V4A patch text sections →
`{oldText,newText}` diff list). A chat dir wins over a transcript on the
shared id; `getById` returns whichever decode has more nodes. Precedence:
`metaJson.cwd` > checkpoint workspace > decoded transcript slug; timestamps
`metaJson.createdAtMs/updatedAtMs` > `meta.createdAt` > file mtime. Wire JSON
is camelCase with absent-when-none `Option` fields.
