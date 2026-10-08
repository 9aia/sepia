#!/usr/bin/env bun
/**
 * Extract golden fixtures for the Devin (sqlite `sessions.db`) and Cline
 * (data-dir) store adapters into `crates/sepia-testkit/fixtures/`.
 *
 * For each case the script builds a real store through the adapter's own
 * write path where one exists (`SqliteStorage.save` for Devin,
 * `ClineStore.install` for the Cline "installed" case) or writes store-native
 * rows/files by hand where that exercises read paths the writer never emits
 * (chisel extensions, `tool_call_state`, malformed rows). It then reads the
 * store back through the adapter's `SessionRepository` and records the wire
 * JSON (`Conversion.sessionToJson`) as `list.json` / `export.<id>.json`.
 *
 * Layout written per case (see crates/sepia-testkit/src/fixtures.rs):
 *   devin/<case>/store.sql              — full dump, rusqlite execute_batch-able
 *   devin/<case>/list.json              — sessionToJson of every listed session
 *   devin/<case>/export.<id>.json       — sessionToJson of getById
 *   cline/<case>/store/sessions/<id>/…  — manifest + transcript verbatim
 *   cline/<case>/index.sql              — dump recreating store/db/sessions.db
 *   cline/<case>/list.json, export.<id>.json
 *   cline/<case>/export-normalized.<id>.json — ids canonicalized (see below)
 *
 * Portability notes baked into the fixtures:
 *  - Cline manifests always carry `"messages_path": ""` so the reader uses
 *    the `<id>.messages.json` sibling — real stores hold absolute paths,
 *    which cannot be checked into a fixture.
 *  - Cline `getById` mints fresh `chatcmpl-tool-<16 hex>` tool-call ids on
 *    every read, so `export.<id>.json` is not reproducible byte-for-byte;
 *    `export-normalized.<id>.json` maps each distinct id to `call-<n>` in
 *    first-seen order for cross-language comparison.
 *  - `index.sql` uses the literal `{{DATA_DIR}}` where a real index row
 *    would store an absolute `messages_path`.
 *
 * Run: bun tools/extract-golden-devin-cline.ts
 */
import { Database } from "bun:sqlite";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// The repo uses bun's isolated workspace layout — `tools/` is not a
// workspace package, so bare specifiers (`effect`, `@effect/*`, `sepia-*`)
// don't resolve here. Import through packages/convert's node_modules (all
// symlink to the same node_modules/.bun targets, so module identity holds)
// and the package entrypoints relatively.
import { Effect, Layer, Option } from "../packages/convert/node_modules/effect/dist/esm/index.js";
import * as BunFileSystem from "../packages/convert/node_modules/@effect/platform-bun/dist/esm/BunFileSystem.js";
import * as BunPath from "../packages/convert/node_modules/@effect/platform-bun/dist/esm/BunPath.js";
import {
  MessageNode,
  PromptHistoryEntry,
  Session,
  Shared,
  ToolCall,
} from "../packages/sepia/src/index.ts";
import { openSessionsDb, SqliteStorage } from "../packages/devin/src/index.ts";
import { Cline, ClineIndex, ClineRepository } from "../packages/cline/src/index.ts";
import { ClineStore, Conversion } from "../packages/convert/src/index.ts";

// ---------------------------------------------------------------------------
// Paths / small helpers
// ---------------------------------------------------------------------------

const REPO_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const FIXTURES = join(REPO_ROOT, "crates", "sepia-testkit", "fixtures");
const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const writeJson = (path: string, value: unknown) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(value, null, 2) + "\n");
};

const writeText = (path: string, text: string) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
};

const iso = (epochSeconds: number): string => new Date(epochSeconds * 1000).toISOString();

/**
 * Canonicalize the `chatcmpl-tool-<rand>` ids the Cline reader mints per call,
 * so Rust output (which mints its own) can be compared after the same
 * normalization. First-seen order across the serialized JSON → `call-<n>`.
 */
const normalizeToolIds = (text: string): string => {
  const ids = new Map<string, string>();
  return text.replace(/chatcmpl-tool-[0-9a-f]+/g, (match) => {
    let canonical = ids.get(match);
    if (canonical === undefined) {
      canonical = `call-${ids.size}`;
      ids.set(match, canonical);
    }
    return canonical;
  });
};

// ---------------------------------------------------------------------------
// SQL dump — text form of a sqlite db that `rusqlite::Connection::execute_batch`
// can replay into a fresh file.
// ---------------------------------------------------------------------------

const sqlLiteral = (value: unknown): string => {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "bigint") return value.toString();
  if (typeof value === "number") return Number.isFinite(value) ? String(value) : "NULL";
  if (value instanceof Uint8Array) return `X'${Buffer.from(value).toString("hex")}'`;
  return `'${String(value).replace(/'/g, "''")}'`;
};

const dumpDb = (dbPath: string, replacements: ReadonlyArray<readonly [string, string]> = []) => {
  const sqlite = new Database(dbPath, { readonly: true });
  try {
    const objects = sqlite
      .query<{ type: string; name: string; sql: string }, []>(
        `SELECT type, name, sql FROM sqlite_master
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
         ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name`,
      )
      .all();

    const out: Array<string> = [
      "-- golden fixture dump (generated by tools/extract-golden-devin-cline.ts)",
      "PRAGMA foreign_keys = OFF;",
      "BEGIN TRANSACTION;",
    ];
    for (const object of objects) out.push(`${object.sql};`);

    const emitRows = (table: string) => {
      const columns = sqlite
        .query<{ name: string }, []>(`PRAGMA table_info("${table}")`)
        .all()
        .map((column) => column.name);
      if (columns.length === 0) return;
      const colList = columns.map((c) => `"${c}"`).join(", ");
      const rows = sqlite
        .query<Record<string, unknown>, []>(`SELECT ${colList} FROM "${table}"`)
        .all();
      for (const row of rows) {
        out.push(
          `INSERT INTO "${table}" (${colList}) VALUES (${columns
            .map((c) => sqlLiteral(row[c]))
            .join(", ")});`,
        );
      }
    };

    for (const object of objects) {
      if (object.type === "table") emitRows(object.name);
    }
    // AUTOINCREMENT tables spawn sqlite_sequence implicitly; keep its counters.
    try {
      emitRows("sqlite_sequence");
    } catch {
      // no AUTOINCREMENT tables — nothing to preserve
    }
    out.push("COMMIT;");

    let text = out.join("\n") + "\n";
    for (const [from, to] of replacements) text = text.split(from).join(to);
    return text;
  } finally {
    sqlite.close();
  }
};

// ---------------------------------------------------------------------------
// Expected-output extraction shared by both adapters
// ---------------------------------------------------------------------------

const recordCase = async (
  agent: "devin" | "cline",
  caseName: string,
  repo: {
    list: () => Effect.Effect<ReadonlyArray<Session>, unknown>;
    getById: (id: string) => Effect.Effect<Option.Option<Session>, unknown>;
  },
  options: { readonly normalizeExports?: boolean } = {},
) => {
  const caseDir = join(FIXTURES, agent, caseName);
  const listed = await Effect.runPromise(repo.list());
  writeJson(
    join(caseDir, "list.json"),
    listed.map((session) => Conversion.sessionToJson(session)),
  );
  for (const summary of listed) {
    const found = await Effect.runPromise(repo.getById(summary.id));
    if (Option.isNone(found)) {
      throw new Error(`${agent}/${caseName}: listed session ${summary.id} not found by getById`);
    }
    const json = JSON.stringify(Conversion.sessionToJson(found.value), null, 2) + "\n";
    writeText(join(caseDir, `export.${summary.id}.json`), json);
    if (options.normalizeExports === true) {
      writeText(join(caseDir, `export-normalized.${summary.id}.json`), normalizeToolIds(json));
    }
  }
  return { caseDir, ids: listed.map((s) => s.id) };
};

// ---------------------------------------------------------------------------
// Devin sessions (IR → SqliteStorage.save → real store bytes)
// ---------------------------------------------------------------------------

const renderedMeta = { summarized_from: 1, num_tokens_preceding: 12, is_system_prefix: null };
const renderedNullMeta = {
  summarized_from: null,
  num_tokens_preceding: null,
  is_system_prefix: null,
};

const devinBasicSessions = (): ReadonlyArray<Session> => [
  Session.make({
    id: "devin-basic-1",
    title: "Basic Devin session",
    workingDirectory: "/work/demo",
    model: "swe-2-high",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_300,
    mainChainId: 1,
    cogsJson: Shared.defaultCogsJson(),
    metadata: Shared.defaultSessionMetadata(),
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "do the thing",
        createdAt: 1_700_000_000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "assistant",
        content: "done — the thing is done",
        createdAt: 1_700_000_100,
        metadata: null,
      }),
    ],
    promptHistory: [
      PromptHistoryEntry.make({ content: "do the thing", timestamp: 1_700_000_000_000 }),
      PromptHistoryEntry.make({
        content: "!git status",
        timestamp: 1_700_000_050_000,
        isShell: true,
      }),
    ],
  }),
  Session.make({
    id: "devin-basic-2",
    title: "Hidden second session",
    workingDirectory: "/work/other",
    agentMode: "plan",
    model: "swe-1-7-medium",
    createdAt: 1_700_001_000,
    lastActivityAt: 1_700_002_000, // newest → first in list order
    mainChainId: 0,
    workspaceDirs: '["/work/other","/work/shared"]',
    hidden: 1,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "plan only, do not touch files",
        createdAt: 1_700_001_000,
        metadata: null,
      }),
    ],
    promptHistory: [
      PromptHistoryEntry.make({
        content: "plan only, do not touch files",
        timestamp: 1_700_001_000_000,
      }),
    ],
  }),
];

const devinFullSessions = (): ReadonlyArray<Session> => [
  Session.make({
    id: "devin-full-1",
    title: "Full-featured session",
    workingDirectory: "/work/app",
    model: "swe-1-7-high",
    createdAt: 1_700_100_000,
    lastActivityAt: 1_700_100_600,
    mainChainId: 6,
    shellLastSeenIndex: 2,
    cogsJson: Shared.defaultCogsJson(),
    // Checkpoints have no column — save() folds them into metadata under
    // "sepia/checkpoints", the read path restores them from there.
    checkpoints: [
      { ref: "a1b2c3d", createdAt: 1_700_100_200_000, runCount: 1, kind: "commit" },
      { ref: "e4f5a6b", createdAt: 1_700_100_400_000, runCount: 2, kind: "stash" },
    ],
    metadata: { ...Shared.defaultSessionMetadata(), custom_note: "golden fixture" },
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "system",
        content: "You are Devin, an AI pair programmer.",
        createdAt: 1_700_100_000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "user",
        content: "wire up the editor",
        // A populated blocks list is the complete ordered block list — the
        // text block rides along so writers can replay the message verbatim.
        blocks: [
          { type: "text", text: "wire up the editor" },
          { type: "image", data: "aGVsbG8gd29ybGQ=", mimeType: "image/png" },
          {
            type: "file",
            uri: "file:///work/app/SPEC.md",
            name: "SPEC.md",
            mimeType: "text/markdown",
            size: 512,
          },
        ],
        createdAt: 1_700_100_050,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 2,
        parentNodeId: Option.some(1),
        role: "assistant",
        content: "",
        thinking: Option.some("I'll update the editor wiring and run the tests."),
        thinkingSignature: Option.some("sealed.v1.Zm9vYmFy"),
        toolCalls: [
          ToolCall.make({
            id: "toolu_edit_1",
            name: "edit",
            arguments: {
              file_path: "/work/app/editor.ts",
              old_string: "const mode = 1;",
              new_string: "const mode = 2;",
            },
            index: 0,
            status: Option.some("pending"),
            locations: [{ path: "/work/app/editor.ts", line: 14 }],
            diffs: [
              {
                path: "/work/app/editor.ts",
                oldText: "const mode = 1;",
                newText: "const mode = 2;",
              },
              // a create rides as a newText-only diff
              { path: "/work/app/new-file.ts", newText: "export {}\n" },
            ],
          }),
          ToolCall.make({
            id: "toolu_exec_1",
            name: "exec",
            arguments: { command: "bun test editor" },
            index: 1,
            status: Option.some("pending"),
          }),
        ],
        usage: Option.some({ input: 4200, output: 88, cacheRead: 1024, cacheWrite: 64 }),
        model: Option.some("swe-1-7-high"),
        requestId: Option.some("req-9abc"),
        finishReason: Option.some("tool_calls"),
        createdAt: 1_700_100_100,
        metadata: renderedMeta,
      }),
      MessageNode.make({
        nodeId: 3,
        parentNodeId: Option.some(2),
        role: "tool",
        content: "Applied edit to /work/app/editor.ts",
        toolCallId: Option.some("toolu_edit_1"),
        toolName: Option.some("edit"),
        toolResult: Option.some({ status: "success", durationMs: 45 }),
        createdAt: 1_700_100_150,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 4,
        parentNodeId: Option.some(3),
        role: "tool",
        content: "FAIL editor.test.ts — 1 test failed",
        toolCallId: Option.some("toolu_exec_1"),
        toolName: Option.some("exec"),
        toolResult: Option.some({ status: "error", exitCode: 1, durationMs: 2_310 }),
        createdAt: 1_700_100_300,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 5,
        parentNodeId: Option.some(4),
        role: "assistant",
        content: "Fixed the race — the suite is green now.",
        thinking: Option.some("The mock timer resolved the ordering issue."),
        thinkingSignature: Option.some("sealed.v1.c2lnYXR1cmU="),
        usage: Option.some({ input: 5_000, output: 12 }),
        finishReason: Option.some("stop"),
        createdAt: 1_700_100_500,
        metadata: renderedNullMeta,
      }),
      MessageNode.make({
        nodeId: 6,
        parentNodeId: Option.some(5),
        role: "assistant",
        content: "",
        // Unsigned thinking is dropped on write — the backend rejects
        // replayed blocks without the provider seal.
        thinking: Option.some("unsigned thought — never reaches the store"),
        createdAt: 1_700_100_600,
        metadata: null,
      }),
    ],
    promptHistory: [
      PromptHistoryEntry.make({ content: "wire up the editor", timestamp: 1_700_100_000_000 }),
      PromptHistoryEntry.make({
        content: "!bun run lint",
        timestamp: 1_700_100_050_000,
        isShell: true,
      }),
    ],
  }),
  Session.make({
    id: "devin-sub-1",
    title: "Research subagent",
    workingDirectory: "/work/app",
    model: "swe-1-7-mini",
    createdAt: 1_700_100_300,
    lastActivityAt: 1_700_100_450,
    mainChainId: 1,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "survey the codebase for the editor",
        createdAt: 1_700_100_300,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "assistant",
        content: "editor lives in src/editor.ts",
        createdAt: 1_700_100_400,
        metadata: null,
      }),
    ],
    promptHistory: [],
  }),
];

/** `subagent_heads` — the parent link a live Devin store keeps per spawned session. */
const seedDevinFullExtras = (dbPath: string) => {
  const sqlite = new Database(dbPath);
  try {
    sqlite.run(`CREATE TABLE IF NOT EXISTS subagent_heads (
      session_id TEXT NOT NULL, agent_id TEXT NOT NULL,
      chain_node_id INTEGER NOT NULL, updated_at INTEGER NOT NULL,
      PRIMARY KEY (session_id, agent_id))`);
    sqlite.run(
      `INSERT INTO subagent_heads (session_id, agent_id, chain_node_id, updated_at)
       VALUES ('devin-full-1', 'devin-sub-1', 2, 1700100300)`,
    );
  } finally {
    sqlite.close();
  }
};

// ---------------------------------------------------------------------------
// Devin "raw" case — hand-written chat_message blobs in the shape a live
// Devin CLI writes (chisel extensions, metrics, thinking objects), plus the
// optional tables (`tool_call_state`, `subagent_heads`, `rendered_commits`)
// and a malformed-but-tolerated row.
// ---------------------------------------------------------------------------

const seedDevinRaw = (dbPath: string) => {
  const sqlite = new Database(dbPath);
  try {
    const cogs = JSON.stringify([
      {
        source: { Session: "System" },
        lifetime: { Unique: "core/model" },
        set_system_prefix: null,
        append_system_messages: [],
        context: [],
        footer_messages: [],
        user_display: [],
        permissions: [],
        tool_availability: null,
        model: "swe-1.5",
      },
    ]);
    sqlite
      .query(
        `INSERT INTO sessions (id, working_directory, backend_type, model, agent_mode,
           created_at, last_activity_at, title, main_chain_id, shell_last_seen_index,
           cogs_json, workspace_dirs, hidden, metadata)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        "devin-raw-1",
        "/work/repo",
        "windsurf",
        "swe-1.5",
        "accept-edits",
        1_700_200_000,
        1_700_200_600,
        "Raw chisel session",
        5,
        2,
        cogs,
        '["/work/repo"]',
        0,
        '{"total_credit_cost":12.5,"total_acu_cost":3}',
      );
    sqlite
      .query(
        `INSERT INTO sessions (id, working_directory, backend_type, model, agent_mode,
           created_at, last_activity_at, title, main_chain_id, shell_last_seen_index,
           cogs_json, workspace_dirs, hidden, metadata)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        "devin-raw-2",
        "/work/repo",
        "windsurf",
        "swe-1.5-mini",
        "accept-edits",
        1_700_200_300,
        1_700_200_300,
        "Sub run (no nodes)",
        0,
        0,
        "[]",
        "[]",
        0,
        "{}",
      );

    const insertNode = sqlite.query(
      `INSERT INTO message_nodes (session_id, node_id, parent_node_id, chat_message, created_at, metadata)
       VALUES (?, ?, ?, ?, ?, ?)`,
    );
    const node = (
      nodeId: number,
      parentNodeId: number | null,
      chatMessage: unknown,
      createdAt: number,
      metadata: unknown = null,
    ) =>
      insertNode.run(
        "devin-raw-1",
        nodeId,
        parentNodeId,
        // A string arg lands in the column verbatim — objects are encoded —
        // so callers can write both well-formed and deliberately broken JSON.
        typeof chatMessage === "string" ? chatMessage : JSON.stringify(chatMessage),
        createdAt,
        metadata === null
          ? null
          : typeof metadata === "string"
            ? metadata
            : JSON.stringify(metadata),
      );

    node(
      0,
      null,
      {
        message_id: "msg-raw-0",
        role: "system",
        content: "You are Devin, an AI software engineer.",
      },
      1_700_200_000,
    );
    node(
      1,
      0,
      {
        message_id: "msg-raw-1",
        role: "user",
        content: "fix the flaky test",
        metadata: {
          num_tokens: null,
          is_user_input: true,
          request_id: null,
          metrics: null,
          finish_reason: null,
          extensions: {},
          created_at: iso(1_700_200_000),
          telemetry: { source: "user", operation: "input" },
        },
      },
      1_700_200_000,
    );
    node(
      2,
      1,
      {
        message_id: "msg-raw-2",
        role: "assistant",
        content: "",
        thinking: {
          thinking: "The test flakes under parallel load — check the shared fixture.",
          signature: "sealed.v1.9f8e7d6c",
        },
        tool_calls: [
          {
            id: "toolu_01",
            name: "exec",
            arguments: { command: "bun test --bail 1" },
            index: 0,
            kind: "function",
          },
          {
            id: "toolu_02",
            name: "read",
            arguments: { file_path: "/work/repo/src/flaky.test.ts" },
            index: 1,
            kind: "function",
          },
        ],
        metadata: {
          num_tokens: 42,
          is_user_input: null,
          request_id: "req-9f",
          metrics: {
            ttft_ms: 812,
            input_tokens: 9_000,
            output_tokens: 42,
            cache_read_tokens: 12_000,
            cache_creation_tokens: 800,
          },
          finish_reason: "tool_calls",
          extensions: {
            "chisel/tool_call_content": {
              toolu_01: {
                toolCallId: "toolu_01",
                title: "Ran command",
                status: "in_progress",
                locations: [],
                kind: "execute",
                rawInput: { command: "bun test --bail 1" },
              },
              toolu_02: {
                toolCallId: "toolu_02",
                title: "Read file",
                status: "completed",
                locations: [{ path: "/work/repo/src/flaky.test.ts", line: 1 }],
                kind: "read",
                rawInput: { file_path: "/work/repo/src/flaky.test.ts" },
              },
            },
          },
          generation_model: "swe-1.5",
          created_at: iso(1_700_200_050),
          telemetry: { source: "assistant", operation: "inference" },
        },
      },
      1_700_200_050,
      { summarized_from: null, num_tokens_preceding: 512, is_system_prefix: null },
    );
    node(
      3,
      2,
      {
        message_id: "msg-raw-3",
        role: "tool",
        content: "1 failed, 9 passed",
        tool_call_id: "toolu_01",
        metadata: {
          num_tokens: null,
          is_user_input: null,
          request_id: null,
          metrics: null,
          finish_reason: null,
          extensions: {
            "chisel/tool_result_meta": { success: false, kind: "exec" },
            "chisel/terminal_output": { exit: { terminal_id: "t0", exit_code: 1 } },
            "chisel/tool_call_timing": { duration_ms: 4_321 },
          },
          created_at: iso(1_700_200_200),
          telemetry: { source: "tool_result", operation: "exec" },
        },
      },
      1_700_200_200,
    );
    node(
      4,
      3,
      {
        message_id: "msg-raw-4",
        role: "tool",
        content: "test('flaky', async () => { /* … */ })",
        tool_call_id: "toolu_02",
        metadata: {
          num_tokens: null,
          is_user_input: null,
          request_id: null,
          metrics: null,
          finish_reason: null,
          extensions: {
            "chisel/tool_result_meta": { success: true, kind: "read" },
          },
          created_at: iso(1_700_200_250),
          telemetry: { source: "tool_result", operation: "read" },
        },
      },
      1_700_200_250,
    );
    node(
      5,
      4,
      {
        message_id: "msg-raw-5",
        role: "assistant",
        content: "Found it — the shared DB fixture races. Serialized it; suite is green.",
        metadata: {
          num_tokens: 61,
          is_user_input: null,
          request_id: "req-a1",
          metrics: {
            ttft_ms: 220,
            input_tokens: 9_100,
            output_tokens: 61,
            cache_read_tokens: null,
            cache_creation_tokens: null,
          },
          finish_reason: "stop",
          extensions: {},
          generation_model: "swe-1.5",
          created_at: iso(1_700_200_600),
          telemetry: { source: "assistant", operation: "inference" },
        },
      },
      1_700_200_600,
    );
    // Malformed-but-tolerated: valid JSON with no role/system-shaped content,
    // and a row metadata column that is not JSON at all.
    node(
      6,
      5,
      '{"content":{"odd":true},"note":"no role, no message_id"}',
      1_700_200_700,
      "not-json{",
    );

    const insertPrompt = sqlite.query(
      `INSERT INTO prompt_history (content, timestamp, session_id, is_shell) VALUES (?, ?, ?, ?)`,
    );
    insertPrompt.run("fix the flaky test", 1_700_200_000_000, "devin-raw-1", 0);
    insertPrompt.run("!bun test --bail 1", 1_700_200_010_000, "devin-raw-1", 1);

    // Optional tables a live Devin store may carry.
    sqlite.run(`CREATE TABLE IF NOT EXISTS tool_call_state (
      session_id TEXT NOT NULL, tool_call_id TEXT NOT NULL,
      tool_call_json TEXT, tool_call_update_json TEXT,
      PRIMARY KEY (session_id, tool_call_id))`);
    // The call row is the still-open snapshot; the update is authoritative.
    // Here it agrees with the tool node's recorded failure (exit_code via
    // _meta["cognition.ai/terminal_exit"]); the toolu_02 row below is a
    // malformed update blob and is skipped on read.
    sqlite.run(
      `INSERT INTO tool_call_state (session_id, tool_call_id, tool_call_json, tool_call_update_json)
       VALUES (?, ?, ?, ?)`,
      "devin-raw-1",
      "toolu_01",
      JSON.stringify({
        toolCallId: "toolu_01",
        title: "Ran command",
        status: "in_progress",
        kind: "execute",
      }),
      JSON.stringify({
        toolCallId: "toolu_01",
        status: "failed",
        _meta: { "cognition.ai/terminal_exit": { exit_code: 1 } },
      }),
    );
    sqlite.run(
      `INSERT INTO tool_call_state (session_id, tool_call_id, tool_call_json, tool_call_update_json)
       VALUES (?, ?, ?, ?)`,
      "devin-raw-1",
      "toolu_02",
      JSON.stringify({
        toolCallId: "toolu_02",
        title: "Read file",
        status: "completed",
        kind: "read",
      }),
      "garbage{not json", // malformed update — skipped, never fatal
    );
    sqlite.run(`CREATE TABLE IF NOT EXISTS subagent_heads (
      session_id TEXT NOT NULL, agent_id TEXT NOT NULL,
      chain_node_id INTEGER NOT NULL, updated_at INTEGER NOT NULL,
      PRIMARY KEY (session_id, agent_id))`);
    sqlite.run(
      `INSERT INTO subagent_heads (session_id, agent_id, chain_node_id, updated_at)
       VALUES ('devin-raw-1', 'devin-raw-2', 2, 1700200300)`,
    );
    sqlite.run(`CREATE TABLE IF NOT EXISTS rendered_commits (
      id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
      sequence_number INTEGER NOT NULL, rendered_html TEXT NOT NULL,
      created_at INTEGER NOT NULL)`);
    sqlite.run(
      `INSERT INTO rendered_commits (session_id, sequence_number, rendered_html, created_at)
       VALUES ('devin-raw-1', 0, '<p>hi</p>', 1700200000)`,
    );
  } finally {
    sqlite.close();
  }
};

// ---------------------------------------------------------------------------
// Devin case driver
// ---------------------------------------------------------------------------

const devinCase = async (
  name: string,
  sessions: ReadonlyArray<Session>,
  extras?: (dbPath: string) => void,
) => {
  const workdir = mkdtempSync(join(tmpdir(), `sepia-golden-devin-${name}-`));
  try {
    const dbPath = join(workdir, "sessions.db");
    const writer = await Effect.runPromise(SqliteStorage.make(dbPath));
    for (const session of sessions) await Effect.runPromise(writer.save(session));
    extras?.(dbPath);

    writeText(join(FIXTURES, "devin", name, "store.sql"), dumpDb(dbPath));

    const reader = await Effect.runPromise(SqliteStorage.make(dbPath, { readonly: true }));
    const { ids } = await recordCase("devin", name, reader);
    console.log(`devin/${name}: sessions [${ids.join(", ")}]`);
  } finally {
    rmSync(workdir, { recursive: true, force: true });
  }
};

// ---------------------------------------------------------------------------
// Cline store builders
// ---------------------------------------------------------------------------

const writeDataDir = (dataDir: string, files: Record<string, unknown>) => {
  for (const [relative, content] of Object.entries(files)) {
    const target = join(dataDir, relative);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(
      target,
      typeof content === "string" ? content : JSON.stringify(content, null, 2) + "\n",
    );
  }
};

const sessionFiles = (
  id: string,
  manifest: Record<string, unknown>,
  messages: Record<string, unknown>,
): Record<string, unknown> => ({
  [`sessions/${id}/${id}.json`]: manifest,
  [`sessions/${id}/${id}.messages.json`]: messages,
});

const clineManifest = (
  id: string,
  overrides: Record<string, unknown> = {},
): Record<string, unknown> => ({
  version: 1,
  session_id: id,
  source: "cli",
  pid: 0,
  cwd: "/work",
  workspace_root: "/work",
  started_at: "2025-01-02T00:00:00.000Z",
  ended_at: "2025-01-02T00:05:00.000Z",
  status: "completed",
  exit_code: 0,
  interactive: true,
  provider: "cline-pass",
  model: "cline-pass/swe-2-high",
  enable_tools: true,
  enable_spawn: true,
  enable_teams: true,
  prompt: "",
  metadata: { title: id },
  messages_path: "", // "" → reader falls back to the <id>.messages.json sibling
  ...overrides,
});

const clineTranscript = (
  id: string,
  messages: ReadonlyArray<unknown>,
  extra: Record<string, unknown> = {},
): Record<string, unknown> => ({
  version: 1,
  updated_at: "2025-01-02T00:05:00.000Z",
  agent: "lead",
  sessionId: id,
  origin: { source: "cli", mode: "user", sessionId: id, version: Cline.CLINE_AGENT_VERSION },
  messages,
  ...extra,
});

/** A `db/sessions.db` index row in the shape the real Cline CLI writes. */
const clineIndexRow = (
  id: string,
  overrides: Record<string, string | number | null> = {},
): Record<string, string | number | null> => ({
  session_id: id,
  source: "cli",
  pid: 0,
  started_at: "2025-01-02T00:00:00.000Z",
  ended_at: "2025-01-02T00:05:00.000Z",
  exit_code: 0,
  status: "completed",
  status_lock: 0,
  interactive: 1,
  provider: Cline.CLINE_PROVIDER,
  model: "cline-pass/swe-2-high",
  cwd: "/work",
  workspace_root: "/work",
  team_name: null,
  enable_tools: 1,
  enable_spawn: 1,
  enable_teams: 1,
  parent_session_id: null,
  parent_agent_id: null,
  agent_id: null,
  conversation_id: null,
  is_subagent: 0,
  prompt: null,
  metadata_json: JSON.stringify({
    sessionHistoryOrigin: { mode: "user", version: Cline.CLINE_AGENT_VERSION },
    source: "cli",
    provider: Cline.CLINE_PROVIDER,
    model: "cline-pass/swe-2-high",
    enableTools: true,
    enableSpawn: true,
    enableTeams: true,
    interactive: true,
    mode: "act",
  }),
  transcript_path: "",
  hook_path: "",
  messages_path: `{{DATA_DIR}}/sessions/${id}/${id}.messages.json`,
  updated_at: "2025-01-02T00:05:00.000Z",
  ...overrides,
});

/** Build a real index db in a scratch dir and return its SQL dump. */
const clineIndexSql = (rows: ReadonlyArray<Record<string, string | number | null>>) => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-golden-cline-index-"));
  try {
    const dbPath = join(dir, "sessions.db");
    const sqlite = openSessionsDb(dbPath, false);
    try {
      sqlite.run(ClineIndex.SESSIONS_DDL);
      for (const row of rows) {
        sqlite.insertSession(
          [...ClineIndex.SESSION_COLUMNS],
          ClineIndex.SESSION_COLUMNS.map((column) => row[column] ?? null),
        );
      }
    } finally {
      sqlite.close();
    }
    return dumpDb(dbPath);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const clineCase = async (
  name: string,
  files: Record<string, unknown>,
  indexRows?: ReadonlyArray<Record<string, string | number | null>>,
) => {
  const caseDir = join(FIXTURES, "cline", name);
  const dataDir = join(caseDir, "store");
  writeDataDir(dataDir, files);
  if (indexRows !== undefined) {
    writeText(join(caseDir, "index.sql"), clineIndexSql(indexRows));
  }
  const repo = ClineRepository.makeClineSessionRepository({ dataDir });
  const { ids } = await recordCase("cline", name, repo, { normalizeExports: true });
  console.log(`cline/${name}: sessions [${ids.join(", ")}]`);
};

// ---------------------------------------------------------------------------
// Cline fixtures
// ---------------------------------------------------------------------------

const clineMultiTurn = () => {
  const id = "1700000000_abcde";
  const files = sessionFiles(
    id,
    clineManifest(id, {
      cwd: "/work",
      workspace_root: "/work",
      started_at: "2025-01-01T00:00:00.000Z",
      ended_at: "2025-01-01T00:05:00.000Z",
      prompt: "fix the flaky test",
      metadata: {
        title: "Fix flaky test",
        checkpointEnabled: true,
        checkpoint: {
          latest: { ref: "c2def", createdAt: 1_700_000_300_000, runCount: 2, kind: "commit" },
          history: [
            { ref: "c1abc", createdAt: 1_700_000_100_000, runCount: 1, kind: "stash" },
            { ref: "c2def", createdAt: 1_700_000_300_000, runCount: 2, kind: "commit" },
          ],
        },
      },
    }),
    clineTranscript(id, [
      {
        id: "msg_0",
        role: "user",
        content: [{ type: "text", text: '<user_input mode="act">fix the flaky test</user_input>' }],
        ts: 1_700_000_000_000,
      },
      {
        id: "msg_1",
        role: "assistant",
        content: [
          {
            type: "thinking",
            thinking: "The test flakes under parallel load — check the shared fixture.",
            signature: "sealed.v1.cline-1",
          },
          { type: "text", text: "I'll read the test and run it." },
          {
            type: "tool_use",
            id: "toolu_r1",
            name: "read_files",
            input: {
              files: [{ path: "/work/src/flaky.test.ts" }, { path: "/work/src/helper.ts" }],
            },
          },
          {
            type: "tool_use",
            id: "toolu_x1",
            name: "run_commands",
            input: { commands: ["bun test"] },
          },
        ],
        ts: 1_700_000_001_000,
        modelInfo: { id: "swe-2-high", provider: "cline-pass" },
        metrics: {
          inputTokens: 5_000,
          outputTokens: 120,
          cacheReadTokens: 3_000,
          cacheWriteTokens: 400,
          cost: 0.0123,
        },
      },
      {
        id: "msg_2",
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "toolu_r1",
            name: "read_files",
            content: [
              { query: "/work/src/flaky.test.ts", result: "test('flaky', …)", success: true },
              { query: "/work/src/helper.ts", result: "export const db = …", success: true },
            ],
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_x1",
            name: "run_commands",
            content: [{ query: "bun test", result: "1 failed, 9 passed", success: false }],
          },
        ],
        ts: 1_700_000_002_000,
      },
      {
        id: "msg_3",
        role: "assistant",
        content: [
          { type: "redacted_thinking", data: "opaque-blob-1" },
          { type: "text", text: "Found the race — patching the fixture." },
          {
            type: "tool_use",
            id: "toolu_e1",
            name: "editor",
            input: { path: "/work/src/helper.ts", old_text: "let x = 0;", new_text: "let x = 1;" },
          },
          {
            type: "tool_use",
            id: "toolu_w1",
            name: "editor",
            input: { path: "/work/src/serial.ts", new_text: "export const lock = 1;\n" },
          },
          {
            type: "tool_use",
            id: "toolu_g1",
            name: "search_codebase",
            input: { queries: ["race"] },
          },
          {
            type: "tool_use",
            id: "toolu_f1",
            name: "fetch_web_content",
            input: { requests: [{ url: "https://example.com/docs" }] },
          },
          { type: "tool_use", id: "toolu_z1", name: "mystery_tool", input: { foo: "bar" } },
        ],
        ts: 1_700_000_003_000,
        modelInfo: { id: "swe-2-high", provider: "cline-pass" },
        metrics: {
          inputTokens: 8_000,
          outputTokens: 200,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          cost: 0,
        },
      },
      {
        id: "msg_4",
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "toolu_e1", name: "editor", content: "edit applied" },
          { type: "tool_result", tool_use_id: "toolu_w1", name: "editor", content: "file created" },
          {
            type: "tool_result",
            tool_use_id: "toolu_g1",
            name: "search_codebase",
            content: [{ query: "race", result: "src/helper.ts:3", success: true }],
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_f1",
            name: "fetch_web_content",
            content: [
              { query: "https://example.com/docs", result: "<html>…</html>", success: true },
            ],
          },
          {
            type: "tool_result",
            tool_use_id: "toolu_z1",
            name: "mystery_tool",
            content: "mystery ok",
          },
        ],
        ts: 1_700_000_004_000,
      },
      {
        id: "msg_5",
        role: "user",
        content: [{ type: "text", text: "looks good, commit it" }],
        ts: 1_700_000_005_000,
      },
      {
        id: "msg_6",
        role: "assistant",
        content: [{ type: "text", text: "Committed as c2def." }],
        ts: 1_700_000_006_000,
        modelInfo: { id: "swe-2-high", provider: "cline-pass" },
        metrics: {
          inputTokens: 9_000,
          outputTokens: 8,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          cost: 0,
        },
      },
    ]),
  );
  return { id, files };
};

const clineAttachments = () => {
  const id = "1700001000_fghij";
  const files = sessionFiles(
    id,
    clineManifest(id, {
      cwd: "/work/pix",
      workspace_root: "/work/pix",
      started_at: "2025-01-02T10:00:00.000Z",
      ended_at: "2025-01-02T10:02:00.000Z",
      prompt: "what's in this image?",
      metadata: { title: "Attachments" },
    }),
    clineTranscript(id, [
      {
        id: "msg_0",
        role: "user",
        content: [
          { type: "text", text: "<user_input>what's in this image?</user_input>" },
          {
            type: "image",
            source: { type: "base64", media_type: "image/png", data: "aGVsbG8=" },
          },
        ],
        ts: 1_700_001_000_000,
      },
      {
        id: "msg_1",
        role: "user",
        content: [
          { type: "text", text: "and this document" },
          {
            type: "document",
            title: "notes.txt",
            source: { type: "text", media_type: "text/plain", text: "the doc body" },
          },
        ],
        ts: 1_700_001_010_000,
      },
      {
        id: "msg_2",
        role: "assistant",
        content: [{ type: "text", text: "The image says hello; the doc is a note." }],
        ts: 1_700_001_020_000,
        modelInfo: { id: "swe-2-high", provider: "cline-pass" },
        metrics: {
          inputTokens: 2_000,
          outputTokens: 14,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          cost: 0.004,
        },
      },
      // An image-only user turn carries no text block — the reader drops it
      // entirely (no user node, no prompt-history entry).
      {
        id: "msg_3",
        role: "user",
        content: [{ type: "image", source: { type: "url", url: "https://example.com/only.png" } }],
        ts: 1_700_001_030_000,
      },
      {
        id: "msg_4",
        role: "user",
        content: [
          { type: "text", text: "one more" },
          { type: "image", source: { type: "url", url: "https://example.com/i.png" } },
        ],
        ts: 1_700_001_040_000,
      },
      {
        id: "msg_5",
        role: "assistant",
        content: [{ type: "text", text: "Same image again." }],
        ts: 1_700_001_050_000,
        modelInfo: { id: "swe-2-high", provider: "cline-pass" },
        metrics: {
          inputTokens: 2_100,
          outputTokens: 5,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          cost: 0,
        },
      },
    ]),
  );
  return { id, files };
};

const clineSubagent = () => {
  const parent = "1700002000_klmno";
  const spawned = `${parent}__agent_researcher`;
  const teamtask = `${parent}__teamtask__scout__x7z9q`;
  const files: Record<string, unknown> = {
    ...sessionFiles(
      parent,
      clineManifest(parent, {
        cwd: "/work/team",
        workspace_root: "/work/team",
        started_at: "2025-01-03T00:00:00.000Z",
        ended_at: "2025-01-03T00:10:00.000Z",
        prompt: "split the work",
        metadata: { title: "Parent session" },
      }),
      clineTranscript(parent, [
        {
          id: "msg_0",
          role: "user",
          content: [{ type: "text", text: "split the work between subagents" }],
          ts: 1_700_002_000_000,
        },
        {
          id: "msg_1",
          role: "assistant",
          content: [{ type: "text", text: "Spawned a researcher and a scout." }],
          ts: 1_700_002_001_000,
          modelInfo: { id: "swe-2-high", provider: "cline-pass" },
          metrics: {
            inputTokens: 1_000,
            outputTokens: 10,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
            cost: 0,
          },
        },
      ]),
    ),
    ...sessionFiles(
      spawned,
      clineManifest(spawned, {
        cwd: "/work/team",
        workspace_root: "/work/team",
        started_at: "2025-01-03T00:01:00.000Z",
        ended_at: "2025-01-03T00:08:00.000Z",
        prompt: "survey the api",
        metadata: { title: "Researcher subagent" },
      }),
      clineTranscript(spawned, [
        {
          id: "msg_0",
          role: "user",
          content: [{ type: "text", text: "survey the api surface" }],
          ts: 1_700_002_060_000,
        },
        {
          id: "msg_1",
          role: "assistant",
          content: [{ type: "text", text: "api surface is small — 3 endpoints." }],
          ts: 1_700_002_120_000,
          modelInfo: { id: "swe-2-high", provider: "cline-pass" },
          metrics: {
            inputTokens: 500,
            outputTokens: 9,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
            cost: 0,
          },
        },
      ]),
    ),
    ...sessionFiles(
      teamtask,
      clineManifest(teamtask, {
        cwd: "/work/team",
        workspace_root: "/work/team",
        started_at: "2025-01-03T00:02:00.000Z",
        ended_at: "2025-01-03T00:09:00.000Z",
        prompt: "scout the tests",
        metadata: { title: "Scout team task" },
      }),
      clineTranscript(teamtask, [
        {
          id: "msg_0",
          role: "user",
          content: [{ type: "text", text: "scout the test layout" }],
          ts: 1_700_002_180_000,
        },
        {
          id: "msg_1",
          role: "assistant",
          content: [{ type: "text", text: "tests live under tests/." }],
          ts: 1_700_002_240_000,
          modelInfo: { id: "swe-2-high", provider: "cline-pass" },
          metrics: {
            inputTokens: 400,
            outputTokens: 6,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
            cost: 0,
          },
        },
      ]),
    ),
  };
  return { parent, spawned, teamtask, files };
};

const clineMalformed = () => {
  const id = "1700003000_pqrst";
  const broken = "1700003100_broken";
  const noManifest = "1700003200_nomanifest";
  const files: Record<string, unknown> = {
    ...sessionFiles(
      id,
      clineManifest(id, {
        cwd: "/work/messy",
        workspace_root: "/work/messy",
        started_at: "2025-01-04T00:00:00.000Z",
        ended_at: "2025-01-04T00:03:00.000Z",
        prompt: "run the suite",
        metadata: { title: "Messy transcript" },
      }),
      clineTranscript(
        id,
        [
          {
            id: "m0",
            role: "user",
            content: [{ type: "text", text: '<user_input mode="act">run the suite</user_input>' }],
            ts: 1_700_003_000_000,
          },
          // a system-role entry inside the transcript is ignored by the reader
          {
            id: "m1",
            role: "system",
            content: [{ type: "text", text: "ignored" }],
            ts: 1_700_003_001_000,
          },
          // assistant content that isn't an array → an empty assistant twin pair
          { id: "m2", role: "assistant", content: "not-an-array", ts: "not-a-number" },
          // an orphaned tool_result (no assistant declared the tool_use) →
          // folded into a "[tool output]" user node
          {
            id: "m3",
            role: "user",
            content: [
              {
                type: "tool_result",
                tool_use_id: "orphan-1",
                name: "run_commands",
                content: "stale output from a compacted call",
              },
            ],
            ts: 1_700_003_003_000,
          },
          // a call whose list field is unreadable is kept raw, not dropped
          {
            id: "m4",
            role: "assistant",
            content: [
              { type: "text", text: "trying" },
              { type: "tool_use", id: "toolu_bad", name: "run_commands", input: { commands: 42 } },
            ],
            ts: 1_700_003_004_000,
          },
          {
            id: "m5",
            role: "user",
            content: [
              {
                type: "tool_result",
                tool_use_id: "toolu_bad",
                name: "run_commands",
                content: "it ran anyway",
              },
            ],
            ts: 1_700_003_005_000,
          },
          // user content that isn't an array → ignored entirely
          { id: "m6", role: "user", content: "a bare string, not an array", ts: 1_700_003_006_000 },
          // an empty text block is skipped like a missing one
          { id: "m7", role: "user", content: [{ type: "text", text: "" }], ts: 1_700_003_007_000 },
          {
            id: "m8",
            role: "user",
            content: [{ type: "text", text: "final question" }],
            ts: 1_700_003_008_000,
          },
        ],
        // top-level fields the reader never modeled survive untouched
        { custom_field: { nested: true } },
      ),
    ),
    // a session whose manifest is not JSON at all → skipped by list();
    // getById on it is a defect, so no export fixture exists for it.
    [`sessions/${broken}/${broken}.json`]: "{not json{",
    [`sessions/${broken}/${broken}.messages.json`]: '{"version":1,"messages":[]}',
    // a dir with no manifest json → skipped by list()
    [`sessions/${noManifest}/${noManifest}.messages.json`]: '{"version":1,"messages":[]}',
    // a non-directory entry inside sessions/ is ignored
    "sessions/stray-file.txt": "not a session dir",
  };
  return { id, files };
};

const clineInstalled = async () => {
  const caseDir = join(FIXTURES, "cline", "installed");
  const dataDir = join(caseDir, "store");
  const id = "1700004000_uvwxy";

  const session = Session.make({
    id: "devin-source-1",
    title: "Installed from a Devin session",
    workingDirectory: "/work/install",
    model: "swe-2-high",
    createdAt: 1_700_004_000,
    lastActivityAt: 1_700_004_500,
    mainChainId: 4,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "system",
        content: "You are Devin.",
        createdAt: 1_700_004_000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "user",
        content: "add a hello endpoint",
        createdAt: 1_700_004_010,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 2,
        parentNodeId: Option.some(1),
        role: "assistant",
        content: "on it",
        toolCalls: [
          ToolCall.make({
            id: "call_1",
            name: "read",
            arguments: { file_path: "/work/install/server.ts" },
          }),
        ],
        createdAt: 1_700_004_020,
        metadata: renderedMeta,
      }),
      MessageNode.make({
        nodeId: 3,
        parentNodeId: Option.some(2),
        role: "tool",
        content: "export const app = …",
        toolCallId: Option.some("call_1"),
        toolName: Option.some("read"),
        toolResult: Option.some({ status: "success" }),
        createdAt: 1_700_004_030,
        metadata: { toolArguments: { file_path: "/work/install/server.ts" } },
      }),
      MessageNode.make({
        nodeId: 4,
        parentNodeId: Option.some(3),
        role: "assistant",
        content: "added GET /hello",
        thinking: Option.some("kept it minimal"),
        thinkingSignature: Option.some("sealed.v1.aW5zdGFsbA=="),
        createdAt: 1_700_004_040,
        metadata: renderedMeta,
      }),
    ],
    promptHistory: [
      PromptHistoryEntry.make({ content: "add a hello endpoint", timestamp: 1_700_004_010_000 }),
    ],
  });

  const store = ClineStore.make(openSessionsDb, dataDir);
  await Effect.runPromise(store.install(session, id).pipe(Effect.provide(fsLayer)));

  // install() writes an absolute messages_path — replace it with "" so the
  // fixture resolves the sibling transcript portably, and dump the index db
  // to index.sql with the data dir as a {{DATA_DIR}} placeholder.
  const manifestPath = join(dataDir, "sessions", id, `${id}.json`);
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  manifest.messages_path = "";
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");

  const dbPath = join(dataDir, "db", "sessions.db");
  writeText(join(caseDir, "index.sql"), dumpDb(dbPath, [[dataDir, "{{DATA_DIR}}"]]));
  rmSync(join(dataDir, "db"), { recursive: true, force: true });

  const repo = ClineRepository.makeClineSessionRepository({ dataDir });
  const { ids } = await recordCase("cline", "installed", repo, { normalizeExports: true });
  console.log(`cline/installed: sessions [${ids.join(", ")}]`);
};

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

rmSync(join(FIXTURES, "devin"), { recursive: true, force: true });
rmSync(join(FIXTURES, "cline"), { recursive: true, force: true });

await devinCase("basic", devinBasicSessions());
await devinCase("full", devinFullSessions(), seedDevinFullExtras);
await devinCase("raw-chisel", [], seedDevinRaw);

{
  const { id, files } = clineMultiTurn();
  await clineCase("multi-turn", files, [clineIndexRow(id, { prompt: "fix the flaky test" })]);
}
{
  const { id, files } = clineAttachments();
  await clineCase("attachments", files, [clineIndexRow(id, { prompt: "what's in this image?" })]);
}
{
  const { parent, spawned, teamtask, files } = clineSubagent();
  await clineCase("subagent", files, [
    clineIndexRow(parent, { prompt: "split the work" }),
    clineIndexRow(spawned, {
      prompt: "survey the api",
      parent_session_id: parent,
      agent_id: "agent_researcher",
      is_subagent: 1,
    }),
    clineIndexRow(teamtask, {
      prompt: "scout the tests",
      parent_session_id: parent,
      agent_id: "scout",
      is_subagent: 1,
    }),
  ]);
}
{
  const { id, files } = clineMalformed();
  await clineCase("malformed", files, [clineIndexRow(id, { prompt: "run the suite" })]);
}
await clineInstalled();

console.log(`\nFixtures written under ${FIXTURES}`);
