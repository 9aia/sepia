#!/usr/bin/env bun
/**
 * Golden-fixture extractor for the Claude Code and Cursor store adapters.
 *
 * Builds synthetic stores (nothing real — no `~/.claude`/`~/.cursor` reads),
 * runs each adapter's `SessionRepository` `list()` + `getById()` over them,
 * and writes the `SessionJson` wire payloads (`Conversion.sessionToJson`) a
 * Rust reimplementation must reproduce:
 *
 *   crates/sepia-testkit/fixtures/
 *     claude/<case>/store/**          # ~/.claude-analogue (projects/, file-history/)
 *     claude/<case>/list.json         # sessionToJson of repo.list()
 *     claude/<case>/export.<id>.json  # sessionToJson of repo.getById(id)
 *     cursor/<case>/store/**          # ~/.cursor-analogue (chats/, projects/);
 *                                     # store.db ships as a text store.sql dump
 *     cursor/<case>/list.json
 *     cursor/<case>/export.<id>.json
 *
 * Determinism: every jsonl entry carries a `timestamp`, every cursor
 * transcript gets a fixed mtime (the only clock the lossy projection has),
 * and every chat dir carries a `meta.json`/`meta['0']` creation stamp, so no
 * output field falls back to `Date.now()`.
 *
 * Usage: bun tools/extract-golden-claude-cursor.ts
 */
import { Database } from "bun:sqlite";
import { createRequire } from "node:module";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import * as ClaudeCodeRepository from "../packages/claude/src/ClaudeCodeRepository.js";
import * as CursorRepository from "../packages/cursor/src/CursorRepository.js";
import * as Cursor from "../packages/cursor/src/Cursor.js";
import { sessionToJson } from "../packages/convert/src/Conversion.js";

const repoRoot = resolve(import.meta.dir, "..");
const fixturesRoot = join(repoRoot, "crates/sepia-testkit/fixtures");

// `tools/` has no node_modules of its own (bun isolated installs), so resolve
// `effect` through a package that depends on it — the ESM entry the adapters
// themselves load, so `Effect.runPromise` runs the same module instance.
const requireFromClaude = createRequire(join(repoRoot, "packages/claude/package.json"));
const effectCjsEntry = requireFromClaude.resolve("effect");
const effectEsmEntry = join(dirname(effectCjsEntry), "../esm/index.js");
const { Effect, Option } = (await import(pathToFileURL(effectEsmEntry).href)) as {
  Effect: { runPromise: <A>(effect: unknown) => Promise<A> };
  Option: {
    isSome: (option: unknown) => boolean;
    getOrThrow: (option: unknown) => unknown;
  };
};

type Session = Parameters<typeof sessionToJson>[0];
interface RepositoryLike {
  list: () => unknown;
  getById: (id: string) => unknown;
}

/* ------------------------------------------------------------------ */
/* small fs helpers                                                    */
/* ------------------------------------------------------------------ */

const clean = (dir: string): void => {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
};

const writeText = (root: string, rel: string, content: string): string => {
  const filePath = join(root, rel);
  mkdirSync(dirname(filePath), { recursive: true });
  writeFileSync(filePath, content);
  return filePath;
};

const writeJson = (filePath: string, value: unknown): void => {
  mkdirSync(dirname(filePath), { recursive: true });
  writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
};

/** Store sidecars ride as pretty JSON so `vp check`'s formatter leaves them alone. */
const writePrettyJson = (root: string, rel: string, compactJson: string): string =>
  writeText(root, rel, `${JSON.stringify(JSON.parse(compactJson), null, 2)}\n`);

const jsonl = (entries: ReadonlyArray<Record<string, unknown> | string>): string =>
  entries.map((entry) => (typeof entry === "string" ? entry : JSON.stringify(entry))).join("\n") +
  "\n";

/** Fixed mtime — cursor transcripts derive every timestamp from it. */
const stamp = (filePath: string, ms: number): void => {
  utimesSync(filePath, new Date(ms), new Date(ms));
};

/* ------------------------------------------------------------------ */
/* extraction                                                          */
/* ------------------------------------------------------------------ */

const runRepo = async (repo: RepositoryLike, caseDir: string): Promise<void> => {
  const sessions = (await Effect.runPromise(repo.list())) as ReadonlyArray<Session>;
  writeJson(
    join(caseDir, "list.json"),
    sessions.map((session) => sessionToJson(session)),
  );
  for (const session of sessions) {
    const found = (await Effect.runPromise(repo.getById(session.id))) as {
      readonly _tag: string;
      readonly value?: Session;
    };
    if (Option.isSome(found)) {
      writeJson(
        join(caseDir, `export.${session.id}.json`),
        sessionToJson(Option.getOrThrow(found) as Session),
      );
    }
  }
};

/* ------------------------------------------------------------------ */
/* claude entry builders                                               */
/* ------------------------------------------------------------------ */

const T = (seconds: number): string => `2026-01-01T00:00:${String(seconds).padStart(2, "0")}.000Z`;

interface ClaudeCtx {
  readonly sessionId: string;
  readonly cwd: string;
}

const claudeUser = (
  ctx: ClaudeCtx,
  uuid: string,
  parentUuid: string | null,
  content: unknown,
  timestamp: string,
  extra: Record<string, unknown> = {},
): Record<string, unknown> => ({
  type: "user",
  uuid,
  parentUuid,
  sessionId: ctx.sessionId,
  cwd: ctx.cwd,
  userType: "external",
  isSidechain: false,
  timestamp,
  message: { role: "user", content },
  ...extra,
});

const claudeAssistant = (
  ctx: ClaudeCtx,
  uuid: string,
  parentUuid: string | null,
  content: ReadonlyArray<unknown>,
  timestamp: string,
  message: Record<string, unknown> = {},
  extra: Record<string, unknown> = {},
): Record<string, unknown> => ({
  type: "assistant",
  uuid,
  parentUuid,
  sessionId: ctx.sessionId,
  cwd: ctx.cwd,
  userType: "external",
  isSidechain: false,
  requestId: `req_${uuid}`,
  timestamp,
  message: {
    id: `msg_${uuid}`,
    type: "message",
    role: "assistant",
    model: "claude-opus-4-5",
    content,
    stop_reason: "end_turn",
    ...message,
  },
  ...extra,
});

/* ------------------------------------------------------------------ */
/* claude cases                                                        */
/* ------------------------------------------------------------------ */

const buildClaudeFullSession = (store: string): void => {
  const ctx: ClaudeCtx = {
    sessionId: "11111111-1111-4111-8111-111111111111",
    cwd: "/work/proj",
  };
  const slug = "-work-proj";
  writeText(
    store,
    `projects/${slug}/${ctx.sessionId}.jsonl`,
    jsonl([
      { type: "summary", summary: "Fix the login bug", leafUuid: "u9" },
      {
        type: "system",
        subtype: "init",
        uuid: "s0",
        parentUuid: null,
        sessionId: ctx.sessionId,
        cwd: ctx.cwd,
        gitBranch: "main",
        version: "2.1.0",
        permissionMode: "acceptEdits",
        slug: "proj",
        timestamp: T(0),
      },
      claudeUser(ctx, "u1", null, "fix the login bug please", T(0), {
        gitBranch: "main",
        version: "2.1.0",
        slug: "proj",
      }),
      // `progress` emits no node but sits in the uuid chain.
      {
        type: "progress",
        data: { type: "hook_progress", hookName: "session-start" },
        uuid: "p1",
        parentUuid: "u1",
        sessionId: ctx.sessionId,
        timestamp: T(1),
      },
      claudeAssistant(
        ctx,
        "u2",
        "p1", // resolves through the progress entry to u1's node
        [
          { type: "thinking", thinking: "look at the auth flow first", signature: "sig-abc" },
          { type: "text", text: "I'll check the auth module." },
          {
            type: "tool_use",
            id: "toolu_1",
            name: "Edit",
            input: {
              file_path: "/work/proj/src/auth.ts",
              old_string: "let retries = 1;",
              new_string: "let retries = 3;",
            },
          },
          {
            type: "tool_use",
            id: "toolu_2",
            name: "Read",
            input: { file_path: "/work/proj/src/login.ts" },
          },
        ],
        T(1),
        {
          stop_reason: "tool_use",
          usage: {
            input_tokens: 1200,
            output_tokens: 340,
            cache_read_input_tokens: 800,
            cache_creation_input_tokens: 64,
          },
        },
      ),
      // two tool results + a toolUseResult sidecar in one user entry
      claudeUser(
        ctx,
        "u3",
        "u2",
        [
          {
            type: "tool_result",
            tool_use_id: "toolu_1",
            content: "The file /work/proj/src/auth.ts has been updated.",
            is_error: false,
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_2",
            content: [
              { type: "text", text: "export function login() {" },
              { type: "text", text: "  // …" },
            ],
          },
        ],
        T(2),
        { toolUseResult: { status: "completed", returnCode: 0 } },
      ),
      // file-history checkpoint + a same-ref isSnapshotUpdate that merges files
      {
        type: "file-history-snapshot",
        messageId: "msg_u2",
        uuid: "snap-1",
        parentUuid: "u3",
        sessionId: ctx.sessionId,
        timestamp: T(3),
        snapshot: {
          messageId: "msg_u2",
          timestamp: "2026-01-01T00:00:03.500Z",
          trackedFileBackups: {
            "/work/proj/src/auth.ts": {
              backupFileName: "hash1@v1",
              version: 1,
              backupTime: "2026-01-01T00:00:03.000Z",
            },
            "/work/proj/src/deleted.ts": { backupFileName: null, version: 2 },
            "/work/proj/src/junk.ts": "not-an-object",
          },
        },
      },
      {
        type: "file-history-snapshot",
        messageId: "msg_u2",
        isSnapshotUpdate: true,
        uuid: "snap-2",
        parentUuid: "snap-1",
        sessionId: ctx.sessionId,
        timestamp: T(3),
        snapshot: {
          messageId: "msg_u2",
          trackedFileBackups: {
            "/work/proj/src/login.ts": { backupFileName: "hash2@v1", version: 1 },
          },
        },
      },
      // user entry carrying image + document attachments -> `blocks`
      claudeUser(
        ctx,
        "u4",
        "snap-2", // resolves through both snapshots to u3's last node
        [
          { type: "text", text: "here is the failure screenshot and notes" },
          {
            type: "image",
            source: { type: "base64", media_type: "image/png", data: "iVBORw0KGgo=" },
          },
          {
            type: "document",
            source: { type: "text", media_type: "text/plain", text: "stack: boom at auth.ts:42" },
            title: "notes.txt",
          },
          { type: "image", source: { type: "url", url: "https://x.invalid/img.png" } },
        ],
        T(4),
      ),
      claudeAssistant(
        ctx,
        "u5",
        "u4",
        [
          { type: "redacted_thinking", data: "opaque-blob-xyz" },
          { type: "text", text: "Now updating the retry loop and README." },
          {
            type: "tool_use",
            id: "toolu_3",
            name: "MultiEdit",
            input: {
              file_path: "/work/proj/src/auth.ts",
              edits: [
                { old_string: "if (fail)", new_string: "if (fail && retry)" },
                { old_string: "throw e;" },
                { not_a_hunk: true },
              ],
            },
          },
          {
            type: "tool_use",
            id: "toolu_4",
            name: "Write",
            input: { file_path: "/work/proj/NOTES.md", content: "# notes\n" },
          },
          { type: "tool_use", id: "toolu_5", name: "Bash", input: { command: "ls" } },
        ],
        T(5),
        {
          stop_reason: "tool_use",
          // nested ephemeral cache tiers instead of cache_creation_input_tokens
          usage: {
            input_tokens: 2100,
            output_tokens: 500,
            cache_creation: {
              ephemeral_5m_input_tokens: 300,
              ephemeral_1h_input_tokens: 120,
            },
          },
        },
      ),
      claudeUser(
        ctx,
        "u6",
        "u5",
        [
          { type: "tool_result", tool_use_id: "toolu_3", content: "Edits applied." },
          {
            type: "tool_result",
            tool_use_id: "toolu_4",
            content: "permission denied",
            is_error: true,
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_5",
            content: [{ type: "image", source: { type: "base64", data: "AA==" } }],
          },
        ],
        T(6),
      ),
      {
        type: "system",
        subtype: "compact_boundary",
        content: "Conversation compacted",
        level: "info",
        compactMetadata: { trigger: "auto", preTokens: 10420 },
        uuid: "s1",
        parentUuid: "u6",
        sessionId: ctx.sessionId,
        cwd: ctx.cwd,
        timestamp: T(7),
      },
      // `/clear` plumbing — kept as a node, excluded from promptHistory
      claudeUser(
        ctx,
        "u8",
        "s1",
        "<command-name>/clear</command-name>\n<command-message>clear</command-message>",
        T(8),
        { isMeta: true },
      ),
      {
        type: "queue-operation",
        operation: "dequeue",
        sessionId: ctx.sessionId,
        uuid: "q1",
        parentUuid: "u8",
        timestamp: T(9),
      },
      claudeAssistant(ctx, "u9", "q1", [{ type: "text", text: "Done." }], T(10), {
        usage: { input_tokens: 50, output_tokens: 10 },
      }),
    ]),
  );
  // Backup blobs the checkpoint refs point at (the IR keeps only the map).
  writeText(store, `file-history/${ctx.sessionId}/hash1@v1`, "let retries = 1;\n");
  writeText(store, `file-history/${ctx.sessionId}/hash2@v1`, "export function login() {}\n");
};

const buildClaudeSubagents = (store: string): void => {
  const parent = "22222222-2222-4222-8222-222222222222";
  const ctx: ClaudeCtx = { sessionId: parent, cwd: "/home/dev/shop" };
  const slug = "-home-dev-shop";
  const sidechain = { isSidechain: true, agentId: "a1b2c3" };
  writeText(
    store,
    `projects/${slug}/${parent}.jsonl`,
    jsonl([
      { type: "summary", summary: "Build the dashboard", leafUuid: "m3" },
      claudeUser(ctx, "m1", null, "build the dashboard page", "2026-02-01T10:00:00.000Z", {
        gitBranch: "feat/dash",
        version: "2.1.0",
        permissionMode: "plan",
        slug: "shop",
      }),
      claudeAssistant(
        ctx,
        "m2",
        "m1",
        [
          { type: "text", text: "Kicking off a research subagent." },
          {
            type: "tool_use",
            id: "toolu_t1",
            name: "Task",
            input: {
              description: "chart research",
              prompt: "find chart libraries",
              subagent_type: "Explore",
            },
          },
        ],
        "2026-02-01T10:00:01.000Z",
        {
          model: "claude-sonnet-4-5",
          stop_reason: "tool_use",
          usage: { input_tokens: 900, output_tokens: 120 },
        },
      ),
      // an inline sidechain root inside the main file — its own tree
      claudeUser(ctx, "s1", null, "inline sidechain root", "2026-02-01T10:00:02.000Z", {
        isSidechain: true,
      }),
      claudeAssistant(
        ctx,
        "m3",
        "m2",
        [{ type: "text", text: "Subagent finished." }],
        "2026-02-01T10:00:03.000Z",
        {
          model: "claude-sonnet-4-5",
          usage: { input_tokens: 300, output_tokens: 40 },
        },
      ),
    ]),
  );
  // current layout: <slug>/<parent>/subagents/agent-<id>.jsonl
  writeText(
    store,
    `projects/${slug}/${parent}/subagents/agent-a1b2c3.jsonl`,
    jsonl([
      {
        ...claudeUser(ctx, "a1", null, "find chart libraries", "2026-02-01T10:00:10.000Z"),
        ...sidechain,
      },
      {
        ...claudeAssistant(
          ctx,
          "a2",
          "a1",
          [
            {
              type: "tool_use",
              id: "toolu_a1",
              name: "Bash",
              input: { command: "npm search chart" },
            },
          ],
          "2026-02-01T10:00:11.000Z",
          { model: "claude-haiku-4-5", stop_reason: "tool_use" },
        ),
        ...sidechain,
      },
      {
        ...claudeUser(
          ctx,
          "a3",
          "a2",
          [
            {
              type: "tool_result",
              tool_use_id: "toolu_a1",
              content: "recharts\nchart.js\n",
            },
          ],
          "2026-02-01T10:00:12.000Z",
        ),
        ...sidechain,
      },
      {
        ...claudeAssistant(
          ctx,
          "a4",
          "a3",
          [{ type: "text", text: "recharts fits." }],
          "2026-02-01T10:00:13.000Z",
          {
            model: "claude-haiku-4-5",
          },
        ),
        ...sidechain,
      },
    ]),
  );
  // legacy layout: agent-*.jsonl as a direct project-dir sibling — the id
  // comes from the filename and entries carry no agentId field.
  writeText(
    store,
    `projects/${slug}/agent-dead99.jsonl`,
    jsonl([
      {
        ...claudeUser(ctx, "b1", null, "summarize findings", "2026-02-01T10:00:20.000Z"),
        isSidechain: true,
      },
      {
        ...claudeAssistant(
          ctx,
          "b2",
          "b1",
          [{ type: "text", text: "two libraries found." }],
          "2026-02-01T10:00:21.000Z",
          {
            model: "claude-haiku-4-5",
          },
        ),
        isSidechain: true,
      },
    ]),
  );
};

const buildClaudeDegraded = (store: string): void => {
  const ctx: ClaudeCtx = {
    sessionId: "33333333-3333-4333-8333-333333333333",
    cwd: "/tmp/scratch",
  };
  writeText(
    store,
    `projects/-tmp-scratch/${ctx.sessionId}.jsonl`,
    jsonl([
      '{"type":"user"', // malformed line mid-file
      '"just a string"', // non-object line
      "", // blank
      // uuid-less user entry — nothing can parent to it
      {
        type: "user",
        parentUuid: null,
        sessionId: ctx.sessionId,
        cwd: ctx.cwd,
        timestamp: "2026-03-01T08:00:00.000Z",
        message: { role: "user", content: "first prompt" },
      },
      // empty string content — plumbing, emits no node
      claudeUser(ctx, "d1", null, "", "2026-03-01T08:00:01.000Z"),
      // message object without content — skipped
      {
        type: "user",
        uuid: "d2",
        parentUuid: "d1",
        timestamp: "2026-03-01T08:00:01.500Z",
        message: { role: "user" },
      },
      // dangling parentUuid chains to the previous node; orphan tool_results
      // keep their output without a resolved tool name
      claudeUser(
        ctx,
        "d3",
        "uuid-that-does-not-exist",
        [
          { type: "tool_result", tool_use_id: "toolu_gone", content: "late output" },
          { type: "tool_result", content: 42 },
        ],
        "2026-03-01T08:00:02.000Z",
      ),
      // snapshot with no messageId — the ref falls back to the entry uuid
      {
        type: "file-history-snapshot",
        uuid: "snap-x",
        parentUuid: "d3",
        sessionId: ctx.sessionId,
        timestamp: "2026-03-01T08:00:02.500Z",
        snapshot: {
          trackedFileBackups: { "/tmp/scratch/a.txt": { backupFileName: "h@v1" } },
        },
      },
      // no ids anywhere — nothing to key a ref under, skipped entirely
      { type: "file-history-snapshot", parentUuid: "snap-x" },
      claudeAssistant(
        ctx,
        "d4",
        "snap-x",
        [{ type: "text", text: "recovered" }],
        "2026-03-01T08:00:03.000Z",
        {
          model: "claude-opus-4-5",
          usage: { input_tokens: 10, output_tokens: 2 },
        },
      ),
      '{"type":"assistant","uuid":"d5","message":{"role":"assis', // truncated tail
    ]),
  );
  // every entry lacks `cwd` — the project-dir slug supplies it (/var/tmp)
  const otherId = "44444444-4444-4444-8444-444444444444";
  writeText(
    store,
    `projects/-var-tmp/${otherId}.jsonl`,
    jsonl([
      {
        type: "user",
        uuid: "e1",
        parentUuid: null,
        sessionId: otherId,
        timestamp: "2026-03-02T09:00:00.000Z",
        message: { role: "user", content: "no cwd anywhere" },
      },
      {
        type: "assistant",
        uuid: "e2",
        parentUuid: "e1",
        sessionId: otherId,
        timestamp: "2026-03-02T09:00:01.000Z",
        message: {
          id: "msg_e2",
          role: "assistant",
          model: "claude-opus-4-5",
          content: [{ type: "text", text: "ok" }],
          stop_reason: "end_turn",
        },
      },
    ]),
  );
};

/* ------------------------------------------------------------------ */
/* cursor store.db builders                                            */
/* ------------------------------------------------------------------ */

const utf8 = (text: string): Uint8Array => new TextEncoder().encode(text);
const hex = (bytes: Uint8Array): string =>
  [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
const msgBlob = (value: unknown): Uint8Array => utf8(JSON.stringify(value));
const blobId = (data: Uint8Array): string => Cursor.blobIdFor(data);
const sqlText = (value: string): string => `'${value.replaceAll("'", "''")}'`;

/**
 * The `.sql` dump is written first and the real `store.db` is materialised
 * from it — the fixture text and the bytes the reader saw can never drift.
 * `rusqlite::Connection::execute_batch` rebuilds the fixture verbatim.
 */
const storeSql = (
  blobs: ReadonlyArray<{ id: string; data: Uint8Array }>,
  metaRow: string | undefined,
  header: string,
): string =>
  [
    `-- ${header}`,
    "-- rebuild: sqlite3 store.db < store.sql  (or rusqlite execute_batch)",
    "PRAGMA user_version = 1;",
    "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);",
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
    ...blobs.map(
      (blob) => `INSERT INTO blobs (id, data) VALUES (${sqlText(blob.id)}, X'${hex(blob.data)}');`,
    ),
    ...(metaRow === undefined
      ? []
      : [`INSERT INTO meta (key, value) VALUES ('0', ${sqlText(metaRow)});`]),
    "",
  ].join("\n");

const materializeStoreDb = (dbPath: string, sql: string): void => {
  mkdirSync(dirname(dbPath), { recursive: true });
  const db = new Database(dbPath);
  try {
    db.exec(sql);
  } finally {
    db.close();
  }
};

const workspaceHash = (cwd: string): string => Cursor.workspaceHashFromCwd(cwd);

/* ------------------------------------------------------------------ */
/* cursor cases                                                        */
/* ------------------------------------------------------------------ */

const CURSOR_CWD = "/home/demo/proj";

const buildCursorChatStore = (store: string): void => {
  const chatId = "55555555-5555-4555-8555-555555555555";
  const wsHash = workspaceHash(CURSOR_CWD);

  const messages = [
    msgBlob({ role: "system", content: "You are an AI coding assistant." }),
    msgBlob({ role: "user", content: "<user_info>\nOS Version: linux\n</user_info>" }),
    msgBlob({
      role: "user",
      content: [{ type: "text", text: "<user_query>\ntake a screenshot\n</user_query>" }],
      providerOptions: { cursor: { requestId: "req-0001" } },
    }),
    msgBlob({
      role: "assistant",
      id: "1",
      content: [
        { type: "redacted-reasoning", data: "opaque-payload" },
        { type: "text", text: "Taking a screenshot." },
        {
          type: "tool-call",
          toolCallId: "tool_1",
          toolName: "Shell",
          args: { command: "grim shot.png" },
        },
      ],
    }),
    msgBlob({
      role: "tool",
      id: "tool_1",
      content: [
        {
          type: "tool-result",
          toolCallId: "tool_1",
          toolName: "Shell",
          result: "Exit code: 0\n\nCommand output:\n\n```\ndone\n```",
        },
      ],
      providerOptions: {
        cursor: {
          highLevelToolCallResult: {
            output: { success: { command: "grim shot.png", executionTime: 5285 }, isError: false },
          },
        },
      },
    }),
    msgBlob({
      role: "user",
      content: [{ type: "text", text: "now crop it" }],
      providerOptions: { cursor: { requestId: "req-0002" } },
    }),
    msgBlob({
      role: "assistant",
      id: "1",
      content: [
        { type: "text", text: "Cropping." },
        {
          type: "tool-call",
          toolCallId: "tool_2",
          toolName: "StrReplace",
          args: {
            path: "/home/demo/proj/src/x.ts",
            old_string: "let a = 1;",
            new_string: "let a = 2;",
          },
        },
      ],
    }),
    msgBlob({
      role: "tool",
      content: [
        {
          type: "tool-result",
          toolCallId: "tool_2",
          result: { error: "edit conflict" },
        },
      ],
      providerOptions: {
        cursor: { highLevelToolCallResult: { output: { isError: true } } },
      },
    }),
  ];
  const opaque = utf8("\x0a\x05not-a-message");
  const dangling = "9".repeat(64);
  const checkpoint = Cursor.encodeCheckpoint({
    messageIds: [...messages.map(blobId), blobId(opaque), dangling],
    workspace: `file://${CURSOR_CWD}`,
    client: "cli",
  });
  const rootId = blobId(checkpoint);
  const emptyBlob = new Uint8Array(0);

  const metaRow = Cursor.encodeStoreMeta({
    agentId: chatId,
    latestRootBlobId: rootId,
    name: "Meta Row Name",
    mode: "agent",
    isRunEverything: true,
    createdAt: 1_700_000_000_000,
    lastUsedModel: "composer-1.5",
  });

  const sql = storeSql(
    [
      { id: blobId(emptyBlob), data: emptyBlob }, // sha256-of-empty, unreferenced
      ...messages.map((data) => ({ id: blobId(data), data })),
      { id: blobId(opaque), data: opaque },
      { id: rootId, data: checkpoint },
    ],
    metaRow,
    `cursor chat-store fixture — chat ${chatId}`,
  );
  const dir = `chats/${wsHash}/${chatId}`;
  writeText(store, `${dir}/store.sql`, sql);
  materializeStoreDb(join(store, dir, "store.db"), sql);
  // metaJson.title wins over meta['0'].name; no cwd -> checkpoint workspace.
  writePrettyJson(
    store,
    `${dir}/meta.json`,
    Cursor.encodeMetaJson({
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_000_500_000,
      title: "Golden store chat",
      hasConversation: true,
    }),
  );
  // oxfmt keeps short arrays on one line — write it that way to stay fmt-clean.
  writeText(store, `${dir}/prompt_history.json`, '["take a screenshot", "now crop it"]\n');

  // the lossy transcript projection of the same chat — the store wins on
  // the shared id in list(), and keeps more nodes in getById().
  const transcript = writeText(
    store,
    `projects/home-demo-proj/agent-transcripts/${chatId}/${chatId}.jsonl`,
    jsonl([
      {
        role: "user",
        message: {
          content: [{ type: "text", text: "<user_query>\ntake a screenshot\n</user_query>" }],
        },
      },
      { role: "assistant", message: { content: [{ type: "text", text: "Done." }] } },
    ]),
  );
  stamp(transcript, 1_700_000_400_000);
};

const buildCursorTranscriptProjection = (store: string): void => {
  const wsHash = workspaceHash(CURSOR_CWD);

  // chat whose store.db was pruned to just the empty blob — the transcript
  // projection holds the real conversation and wins getById().
  const prunedId = "66666666-6666-4666-8666-666666666666";
  const emptyBlob = new Uint8Array(0);
  const emptyId = blobId(emptyBlob);
  const prunedSql = storeSql(
    [{ id: emptyId, data: emptyBlob }],
    Cursor.encodeStoreMeta({
      agentId: prunedId,
      latestRootBlobId: emptyId, // decodes to nothing — zero message refs
      name: "Pruned Chat",
      mode: "default",
      createdAt: 1_700_000_100_000,
      lastUsedModel: "composer-1",
    }),
    `cursor pruned store — chat ${prunedId}`,
  );
  writeText(store, `chats/${wsHash}/${prunedId}/store.sql`, prunedSql);
  materializeStoreDb(join(store, `chats/${wsHash}/${prunedId}/store.db`), prunedSql);
  writePrettyJson(
    store,
    `chats/${wsHash}/${prunedId}/meta.json`,
    Cursor.encodeMetaJson({
      createdAtMs: 1_700_000_100_000,
      updatedAtMs: 1_700_000_200_000,
      title: "Pruned Chat",
      hasConversation: true,
    }),
  );

  const prunedTranscript = writeText(
    store,
    `projects/home-demo-proj/agent-transcripts/${prunedId}/${prunedId}.jsonl`,
    jsonl([
      {
        role: "user",
        message: {
          content: [{ type: "text", text: "<user_query>\nexplore the repo\n</user_query>" }],
        },
      },
      {
        role: "assistant",
        message: {
          content: [
            { type: "text", text: "I'll look around.\n\n[REDACTED]" },
            { type: "tool_use", name: "Glob", input: { glob_pattern: "src/**" } },
            {
              type: "tool_use",
              id: "t9",
              name: "ApplyPatch",
              input: {
                patch:
                  "*** Begin Patch\n" +
                  "*** Update File: /home/demo/proj/a.ts\n" +
                  "@@\n" +
                  " ctx\n" +
                  "-old\n" +
                  "+new\n" +
                  "*** Delete File: /home/demo/proj/dead.ts\n" +
                  "-gone\n" +
                  "*** End Patch",
              },
            },
          ],
        },
      },
      '{"role":"assistant" BAD', // malformed line — skipped
      { type: "turn_ended", status: "error", error: { message: "rate limited" } },
      {
        role: "user",
        message: { content: [{ type: "text", text: "continue please" }] },
      },
      { role: "assistant", message: { content: [{ type: "text", text: "All done." }] } },
      { type: "turn_ended", status: "completed" },
    ]),
  );
  stamp(prunedTranscript, 1_700_000_180_000);

  // transcript-only chat (no chats/ dir at all) plus a subagent file
  const chat2 = "77777777-7777-4777-8777-777777777777";
  const chat2File = writeText(
    store,
    `projects/home-demo-proj/agent-transcripts/${chat2}/${chat2}.jsonl`,
    jsonl([
      {
        role: "user",
        message: {
          content: [{ type: "text", text: "<user_query>\nrelease it\n</user_query>" }],
        },
      },
      {
        role: "assistant",
        message: {
          content: [
            { type: "text", text: "Tagging." },
            {
              type: "tool_use",
              id: "t_rel",
              name: "StrReplace",
              input: { path: "/home/demo/proj/VERSION", old_string: "0.1.0", new_string: "0.2.0" },
            },
          ],
        },
      },
      { type: "turn_ended", status: "completed" },
    ]),
  );
  stamp(chat2File, 1_700_000_300_000);
  const subFile = writeText(
    store,
    `projects/home-demo-proj/agent-transcripts/${chat2}/subagents/sub-99.jsonl`,
    jsonl([
      {
        role: "user",
        message: { content: [{ type: "text", text: "double-check the tag" }] },
      },
      { role: "assistant", message: { content: [{ type: "text", text: "looks right" }] } },
    ]),
  );
  stamp(subFile, 1_700_000_320_000);

  // a chat dir with only meta.json — summarize-only, no store.db at all
  const metaOnlyDir = `chats/${workspaceHash("/home/demo/other")}/88888888-8888-4888-8888-888888888888`;
  const metaJson = writePrettyJson(
    store,
    `${metaOnlyDir}/meta.json`,
    JSON.stringify({
      schemaVersion: 1,
      createdAtMs: 1_699_999_000_000,
      hasConversation: false,
      title: "Empty Chat",
      updatedAtMs: 1_699_999_000_000,
      cwd: "/home/demo/other",
    }),
  );
  stamp(metaJson, 1_699_999_000_000);
};

const buildCursorDegradedStore = (store: string): void => {
  const wsHash = workspaceHash("/home/demo/scratch");

  // meta['0'] as plain (un-hexed) JSON — parseStoreMeta's second candidate
  const plainId = "99999999-9999-4999-8999-999999999999";
  const userMsg = msgBlob({
    role: "user",
    content: [{ type: "text", text: "loose prompt" }],
  });
  const plainCheckpoint = Cursor.encodeCheckpoint({
    messageIds: [blobId(userMsg)],
    workspace: "file:///home/demo/scratch",
  });
  const plainSql = storeSql(
    [
      { id: blobId(userMsg), data: userMsg },
      { id: blobId(plainCheckpoint), data: plainCheckpoint },
    ],
    JSON.stringify({
      agentId: plainId,
      latestRootBlobId: blobId(plainCheckpoint),
      name: "Plain Meta",
      mode: "agent",
      createdAt: 1_700_000_700_000,
      lastUsedModel: "composer-1",
    }),
    `cursor degraded store — chat ${plainId} (meta row is plain JSON, not hex)`,
  );
  writeText(store, `chats/${wsHash}/${plainId}/store.sql`, plainSql);
  materializeStoreDb(join(store, `chats/${wsHash}/${plainId}/store.db`), plainSql);
  writePrettyJson(
    store,
    `chats/${wsHash}/${plainId}/meta.json`,
    Cursor.encodeMetaJson({
      createdAtMs: 1_700_000_700_000,
      updatedAtMs: 1_700_000_700_000,
      title: "Plain Meta",
      hasConversation: true,
      cwd: "/home/demo/scratch",
    }),
  );

  // a schema-less store.db — valid database, no tables; allSafe degrades to []
  const noTablesId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const bareSql = [
    `-- cursor degraded store — chat ${noTablesId} (no blobs/meta tables)`,
    "PRAGMA user_version = 1;",
    "",
  ].join("\n");
  writeText(store, `chats/${wsHash}/${noTablesId}/store.sql`, bareSql);
  materializeStoreDb(join(store, `chats/${wsHash}/${noTablesId}/store.db`), bareSql);
  writePrettyJson(
    store,
    `chats/${wsHash}/${noTablesId}/meta.json`,
    Cursor.encodeMetaJson({
      createdAtMs: 1_700_000_800_000,
      updatedAtMs: 1_700_000_800_000,
      title: "Ghost Chat",
      hasConversation: false,
      cwd: "/home/demo/scratch",
    }),
  );

  // transcript with no chats dir in a second project slug
  const orphan = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
  const orphanFile = writeText(
    store,
    `projects/home-demo-elsewhere/agent-transcripts/${orphan}/${orphan}.jsonl`,
    jsonl([
      { role: "user", message: { content: [{ type: "text", text: "orphan chat" }] } },
      {
        role: "assistant",
        message: {
          content: [
            {
              type: "tool_use",
              id: "to1",
              name: "Write",
              input: { path: "/x/f.ts", contents: "x" },
            },
          ],
        },
      },
    ]),
  );
  stamp(orphanFile, 1_700_000_900_000);
};

/* ------------------------------------------------------------------ */
/* store.db binaries -> .sql only in the committed fixtures            */
/* ------------------------------------------------------------------ */

const dropBinaryDbs = (dir: string): void => {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      dropBinaryDbs(full);
      continue;
    }
    if (/^store\.db(-wal|-shm|-journal)?$/.test(entry)) rmSync(full);
  }
};

/* ------------------------------------------------------------------ */
/* READMEs (written here so reruns keep the fixture dirs whole)         */
/* ------------------------------------------------------------------ */

const CLAUDE_README = `# Golden fixtures — Claude Code adapter

Each case dir holds a synthetic \`~/.claude\` analogue under \`store/\` plus the
adapter's output as \`SessionJson\` wire payloads (\`sessionToJson\` from
\`sepia-convert\`): \`list.json\` is the repository \`list()\` result (summaries,
\`nodes\` empty, sorted by \`lastActivityAt\` desc) and \`export.<id>.json\` the
full \`getById(id)\` session. Cases: \`full-session\` (multi-turn with thinking +
\`redacted_thinking\` signatures, Edit/MultiEdit/Write/Read/Bash tool calls with
locations+diffs, usage incl. flat and nested ephemeral cache tiers,
\`file-history-snapshot\` checkpoints with an \`isSnapshotUpdate\` merge, image /
document blocks, \`system\` init + compact_boundary entries, an \`isMeta\` prompt
excluded from \`promptHistory\`, and \`progress\`/\`queue-operation\` plumbing that
emits no node but stays in the uuid chain); \`subagents\` (main file plus the
current \`<parent>/subagents/agent-*.jsonl\` layout and the legacy
\`agent-*.jsonl\` project-root sibling, \`isSidechain\` entries whose \`sessionId\`
names the parent, \`agentId\` carried on entries or derived from the \`agent-\`
filename, plus an inline \`isSidechain\` root inside the main file);
\`degraded\` (malformed/truncated/non-object lines, missing uuids, dangling
\`parentUuid\` chaining to the previous node, orphan \`tool_result\` blocks, a
uuid-fallback snapshot ref, and a file with no \`cwd\` that falls back to the
decoded project slug).

Layout a Rust reader must handle: \`projects/<slug>/<sessionId>.jsonl\` where the
slug is the cwd with every non-alphanumeric byte flattened to \`-\` (leading
\`-\` kept — \`encodeProjectDir("/work/proj")\` = \`-work-proj\`; decode maps
\`-\` back to \`/\` and ensures a leading \`/\`). Subagent transcripts live at
\`<slug>/<parent-id>/subagents/agent-*.jsonl\` (current) or as \`agent-*.jsonl\`
siblings (legacy). \`file-history/<sessionId>/\` holds backup blobs named by
\`snapshot.trackedFileBackups[path].backupFileName\` — the IR only keeps the
path→backup map on \`metadata.fileHistory\`. Entry taxonomy: \`summary\`
(\`summary\` text → title, no node), \`user\`/\`assistant\` (uuid/parentUuid
chain, \`sessionId\`, \`cwd\`, ISO-millis \`timestamp\` → epoch-seconds floor,
\`gitBranch\`/\`version\`/\`slug\`/\`permissionMode\` → session meta, flags
\`isSidechain\`/\`isMeta\`/\`isCompactSummary\`/\`isVisibleInTranscriptOnly\`,
\`requestId\`/\`toolUseResult\` on tool-result entries), \`system\`
(\`subtype\`/\`level\`/\`compactMetadata\`), \`file-history-snapshot\` (ref =
\`messageId\` else \`uuid\`; \`snapshot.timestamp\` ms-floor → checkpoint
\`createdAt\` in ms), and \`queue-operation\`/\`progress\`/unknown types that
emit no node yet still anchor the uuid chain. \`parentUuid\` resolves through
non-node entries to the nearest emitted node; explicit \`null\` starts a new
root; a dangling id falls back to the previous node. \`message.content\` is a
string or block array: \`text\`/\`image\`/\`document\` → IR blocks,
\`tool_use{id,name,input}\` → ToolCall (Edit/MultiEdit/Write inputs → diffs,
paths → locations), \`tool_result{tool_use_id,content,is_error}\` inside a
\`user\` entry → \`tool\` node paired to the earlier call, \`thinking.signature\`
and \`redacted_thinking.data\` → thinking + \`thinkingSignature\`, and usage
reads \`input_tokens\`/\`output_tokens\`/\`cache_read_input_tokens\` /
\`cache_creation_input_tokens\` or \`cache_creation.ephemeral_5m/1h_input_tokens\`.
Wire JSON is camelCase; \`Option\` fields (\`parentNodeId\`, \`toolCallId\`,
\`thinking\`, …) are absent when none.
`;

const CURSOR_README = `# Golden fixtures — Cursor adapter

Each case dir holds a synthetic \`~/.cursor\` analogue under \`store/\` plus the
adapter's \`SessionJson\` output: \`list.json\` (summaries, \`nodes\` empty,
sorted by \`lastActivityAt\` desc) and \`export.<id>.json\` per listed id.
Cases: \`chat-store\` (a full \`store.db\` chat — system + \`<user_info>\` +
\`<user_query>\` user blobs, redacted-reasoning + text + \`tool-call\` assistant
blobs, tool-result blobs with \`providerOptions.cursor.highLevelToolCallResult\`
success \`executionTime\` → \`durationMs\` and an \`isError\` result, an opaque
binary blob and a dangling checkpoint ref counted as \`opaqueBlobs\`, hex
\`meta['0']\`, \`meta.json\`/\`prompt_history.json\` sidecars, and a thinner
same-id transcript the store beats in \`list()\`/\`getById()\`);
\`transcript-projection\` (transcript-only chat, a pruned store.db whose
transcript wins \`getById\` on node count while \`list()\` still shows the chat
summary, a meta.json-only chat dir, and a \`<chat>/subagents/*.jsonl\`
transcript carrying \`parentSessionId\`); \`degraded-store\` (a plain-JSON —
not hex — \`meta['0']\` row, a schema-less store.db with no tables, and an
orphan transcript in a second project slug).

Layout: \`chats/<workspace-hash>/<chat-id>/\` where the hash dir is \`md5\` of
\`path.resolve(cwd)\`, holding \`store.db\` (shipped here as a \`store.sql\` text
dump — rebuild with \`sqlite3 store.db < store.sql\` or
\`rusqlite::Connection::execute_batch\`; blob literals are \`X'hex'\`), plus
\`meta.json\` and \`prompt_history.json\` sidecars. The store schema is exactly
\`blobs(id TEXT PRIMARY KEY, data BLOB)\` and \`meta(key TEXT PRIMARY KEY, value
TEXT)\` with \`PRAGMA user_version = 1\` (real stores additionally run WAL).
\`meta['0']\` is the hex encoding of a JSON object — plain JSON is tolerated —
with \`agentId\`, \`latestRootBlobId\`, \`name\`, \`mode\`, \`isRunEverything\`,
\`createdAt\` (epoch ms) and \`lastUsedModel\`. \`blobs.id\` is the lowercase
sha256 hex of \`data\`; \`latestRootBlobId\` names a protobuf-ish checkpoint
blob whose field-1 length-delimited 32-byte entries are the ordered message
blob ids, field 9 the \`file://\` workspace URI, field 10 a varint flag, and
field 22 the client tag (\`"cli"\`). Message blobs are AI-SDK JSON:
\`{role:"system",content:string}\`, \`{role:"user",content:string}\` (raw
\`<user_info>\` plumbing → \`metadata.context\`) or \`content:[{type:"text"}]\`
with \`providerOptions.cursor.requestId\` and \`<user_query>\` unwrapping,
\`{role:"assistant",id:"1",content:[{type:"redacted-reasoning",data?},
{type:"text"},{type:"tool-call",toolCallId,toolName,args}]}\`, and
\`{role:"tool",content:[{type:"tool-result",toolCallId,toolName?,result}]}\`.
The second store is the lossy projection
\`projects/<slug>/agent-transcripts/<chat-id>/<chat-id>.jsonl\` (slug = cwd
flattened to \`-\` with leading/trailing dashes trimmed — \`home-demo-proj\`,
decoded by mapping \`-\`→\`/\` and prefixing \`/\`), one
\`{role,message:{content}}\` JSON message per line with \`text\` and
\`tool_use{name,input}\` blocks, \`[REDACTED]\` text markers → thinking, and
\`{"type":"turn_ended","status":"error",error}\` lines → \`metadata.turnErrors\`;
it carries no timestamps, usage or tool results, so every node takes the file
mtime. Tool inputs project to locations/diffs via \`StrReplace\`/\`Edit\`/
\`Write\`/\`Delete\`/\`ApplyPatch\` (V4A patch text sections →
\`{oldText,newText}\` diff list). A chat dir wins over a transcript on the
shared id; \`getById\` returns whichever decode has more nodes. Precedence:
\`metaJson.cwd\` > checkpoint workspace > decoded transcript slug; timestamps
\`metaJson.createdAtMs/updatedAtMs\` > \`meta.createdAt\` > file mtime. Wire JSON
is camelCase with absent-when-none \`Option\` fields.
`;

/* ------------------------------------------------------------------ */
/* main                                                                */
/* ------------------------------------------------------------------ */

const buildCase = async (
  adapter: "claude" | "cursor",
  name: string,
  build: (store: string) => void,
): Promise<void> => {
  const caseDir = join(fixturesRoot, adapter, name);
  const store = join(caseDir, "store");
  clean(caseDir);
  build(store);
  const repo: RepositoryLike =
    adapter === "claude"
      ? ClaudeCodeRepository.makeClaudeCodeSessionRepository({
          projectsDir: join(store, "projects"),
        })
      : CursorRepository.makeCursorSessionRepository({ cursorDir: store });
  await runRepo(repo, caseDir);
  if (adapter === "cursor") dropBinaryDbs(store);
  console.log(`${adapter}/${name}: extracted`);
};

const claudeDir = join(fixturesRoot, "claude");
const cursorDir = join(fixturesRoot, "cursor");
rmSync(claudeDir, { recursive: true, force: true });
rmSync(cursorDir, { recursive: true, force: true });

await buildCase("claude", "full-session", buildClaudeFullSession);
await buildCase("claude", "subagents", buildClaudeSubagents);
await buildCase("claude", "degraded", buildClaudeDegraded);
writeText(claudeDir, "README.md", CLAUDE_README);

await buildCase("cursor", "chat-store", buildCursorChatStore);
await buildCase("cursor", "transcript-projection", buildCursorTranscriptProjection);
await buildCase("cursor", "degraded-store", buildCursorDegradedStore);
writeText(cursorDir, "README.md", CURSOR_README);

console.log(`fixtures written under ${fixturesRoot}`);
