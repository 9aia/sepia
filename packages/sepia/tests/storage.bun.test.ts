/**
 * Bun-only specs: `SessionSqlite` and `SqliteStorage` import `bun:sqlite` at
 * module load, so Node vitest can never execute them — vitest excludes
 * `*.bun.test.ts` and these run under `bun test` (the `test:bun` script, also
 * part of `test`). They also give `ClineStore`/`ClineRepository` a real
 * sessions.db to work against instead of an injected stub.
 */
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import { Effect, Either, Layer, Option } from "effect";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Database } from "bun:sqlite";
import { describe, expect, test } from "vite-plus/test";
import * as ClineIndex from "../src/ClineIndex.js";
import * as ClineRepository from "../src/ClineRepository.js";
import * as ClineStore from "../src/ClineStore.js";
import { MessageNode, PromptHistoryEntry, Session, StorageError } from "../src/Domain.js";
import { openSessionsDb } from "../src/SessionSqlite.js";
import * as SqliteStorage from "../src/SqliteStorage.js";

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const session = Session.make({
  id: "s1",
  title: "Bun storage round-trip",
  workingDirectory: "/work",
  model: "swe-2-high",
  createdAt: 1_700_000_000,
  lastActivityAt: 1_700_000_300,
  mainChainId: 1,
  metadata: null,
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
      content: "done",
      createdAt: 1_700_000_100,
      metadata: null,
    }),
  ],
  promptHistory: [PromptHistoryEntry.make({ content: "do the thing", timestamp: 1_700_000_000 })],
});

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-bun-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

describe("SessionSqlite (real bun:sqlite)", () => {
  test("openSessionsDb creates, writes and reads the session index", () =>
    withTempDir((dir) => {
      const dbPath = join(dir, "sessions.db");
      const row = ClineIndex.sessionRow(session, "s1", join(dir, "s1.messages.json"));

      const writer = openSessionsDb(dbPath, false);
      try {
        writer.run(ClineIndex.SESSIONS_DDL);
        expect(writer.allTables()).toContain("sessions");
        writer.insertSession(
          [...ClineIndex.SESSION_COLUMNS],
          ClineIndex.SESSION_COLUMNS.map((column) => row[column]),
        );
      } finally {
        writer.close();
      }

      const reader = openSessionsDb(dbPath, true);
      try {
        const found = reader.get<{ session_id: string; status: string }>(
          "select session_id, status from sessions where session_id = ?",
          "s1",
        );
        expect(found?.session_id).toBe("s1");
        expect(found?.status).toBe("completed");
        expect(
          reader.get("select session_id from sessions where session_id = ?", "missing"),
        ).toBeNull();
      } finally {
        reader.close();
      }
    }));
});

describe("SqliteStorage (real bun:sqlite)", () => {
  test("saves, reads, lists and deletes a session", async () => {
    const repo = await Effect.runPromise(SqliteStorage.make(":memory:"));

    await Effect.runPromise(repo.save(session));

    const found = await Effect.runPromise(repo.getById("s1"));
    expect(Option.isSome(found)).toBe(true);
    if (Option.isSome(found)) {
      expect(found.value.id).toBe("s1");
      expect(found.value.title).toBe("Bun storage round-trip");
      expect(found.value.nodes.length).toBe(2);
      expect(found.value.nodes[0]?.content).toBe("do the thing");
    }
    expect(Option.isNone(await Effect.runPromise(repo.getById("missing")))).toBe(true);
    expect(await Effect.runPromise(repo.hasSession("s1"))).toBe(true);

    const listed = await Effect.runPromise(repo.list());
    expect(listed.map((s) => s.id)).toEqual(["s1"]);

    await Effect.runPromise(repo.delete("s1"));
    expect(Option.isNone(await Effect.runPromise(repo.getById("s1")))).toBe(true);
  });

  test("truncateSessionNodes drops the node suffix and moves the tail markers", async () =>
    withTempDir(async (dir) => {
      const dbPath = join(dir, "store.db");
      const repo = await Effect.runPromise(SqliteStorage.make(dbPath));
      const full = Session.make({
        id: "s1",
        title: "Rewind me",
        workingDirectory: "/work",
        model: "swe-2-high",
        createdAt: 1_700_000_000,
        lastActivityAt: 1_700_000_400,
        mainChainId: 3,
        metadata: null,
        nodes: [0, 1, 2, 3].map((nodeId) =>
          MessageNode.make({
            nodeId,
            parentNodeId: nodeId === 0 ? Option.none() : Option.some(nodeId - 1),
            role: nodeId % 2 === 0 ? "user" : "assistant",
            content: `node ${nodeId}`,
            createdAt: 1_700_000_000 + nodeId * 100,
            metadata: null,
          }),
        ),
        promptHistory: [
          PromptHistoryEntry.make({ content: "node 0", timestamp: 1_700_000_000 }),
          PromptHistoryEntry.make({ content: "node 2", timestamp: 1_700_000_200 }),
        ],
      });
      await Effect.runPromise(repo.save(full));

      // A live devin store carries extra tables sepia's schema never
      // creates; seed them so the truncate covers every join that exists
      // (and leaves the ones that don't).
      const seed = new Database(dbPath);
      try {
        seed.run(`CREATE TABLE tool_call_state (
          session_id TEXT NOT NULL, tool_call_id TEXT NOT NULL,
          tool_call_json TEXT, tool_call_update_json TEXT,
          PRIMARY KEY (session_id, tool_call_id))`);
        seed.run(`CREATE TABLE subagent_heads (
          session_id TEXT NOT NULL, agent_id TEXT NOT NULL,
          chain_node_id INTEGER NOT NULL, updated_at INTEGER NOT NULL,
          PRIMARY KEY (session_id, agent_id))`);
        seed.run(`CREATE TABLE rendered_commits (
          id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
          sequence_number INTEGER NOT NULL, rendered_html TEXT NOT NULL,
          created_at INTEGER NOT NULL)`);
        seed.run(
          "INSERT INTO tool_call_state VALUES ('s1', 'call-keep', '{}', '{}'), ('s1', 'call-drop', '{}', '{}')",
        );
        seed.run(
          "INSERT INTO subagent_heads VALUES ('s1', 'agent-keep', 1, 0), ('s1', 'agent-drop', 3, 0)",
        );
        seed.run(
          "INSERT INTO rendered_commits (session_id, sequence_number, rendered_html, created_at) " +
            "VALUES ('s1', 0, '<p>a</p>', 0), ('s1', 3, '<p>b</p>', 0)",
        );
      } finally {
        seed.close();
      }

      // In-place rewind: delete nodes 2..3, point the session row at node 1.
      await Effect.runPromise(
        SqliteStorage.truncateSessionNodes(dbPath, "s1", {
          removedNodeIds: [2, 3],
          removedToolCallIds: ["call-drop"],
          lastActivityAt: 1_700_000_100,
          mainChainId: 1,
        }),
      );

      // tool_call_state/subagent_heads join the removed ids; their kept
      // rows survive. rendered_commits keys on `sequence_number`, which no
      // column in message_nodes maps to (see truncateSessionNodes), so
      // both rows stay — this test pins that decision deliberately.
      const check = new Database(dbPath, { readonly: true });
      try {
        expect(
          check
            .query<{ tool_call_id: string }, []>(
              "SELECT tool_call_id FROM tool_call_state WHERE session_id = 's1' ORDER BY tool_call_id",
            )
            .all()
            .map((row) => row.tool_call_id),
        ).toEqual(["call-keep"]);
        expect(
          check
            .query<{ agent_id: string }, []>(
              "SELECT agent_id FROM subagent_heads WHERE session_id = 's1' ORDER BY agent_id",
            )
            .all()
            .map((row) => row.agent_id),
        ).toEqual(["agent-keep"]);
        expect(
          check
            .query<{ c: number }, []>(
              "SELECT COUNT(*) c FROM rendered_commits WHERE session_id = 's1'",
            )
            .get()?.c,
        ).toBe(2);
      } finally {
        check.close();
      }

      // The server holds the store read-only; a second connection sees the cut.
      const ro = await Effect.runPromise(SqliteStorage.make(dbPath, { readonly: true }));
      const found = await Effect.runPromise(ro.getById("s1"));
      expect(Option.isSome(found)).toBe(true);
      if (Option.isSome(found)) {
        expect(found.value.nodes.map((node) => node.nodeId)).toEqual([0, 1]);
        expect(found.value.lastActivityAt).toBe(1_700_000_100);
        expect(found.value.mainChainId).toBe(1);
        // prompt_history is the input log — it is not conversation state
        // and has no node key to truncate by, so every row survives.
        expect(found.value.promptHistory.length).toBe(2);
      }
    }));

  test("a read-only store still reads but refuses writes", async () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-bun-ro-"));
    try {
      const dbPath = join(dir, "store.db");
      const rw = await Effect.runPromise(SqliteStorage.make(dbPath));
      await Effect.runPromise(rw.save(session));

      const ro = await Effect.runPromise(SqliteStorage.make(dbPath, { readonly: true }));
      const found = await Effect.runPromise(ro.getById("s1"));
      expect(Option.isSome(found)).toBe(true);

      const saveResult = await Effect.runPromise(Effect.either(ro.save(session)));
      expect(Either.isLeft(saveResult)).toBe(true);
      const deleteResult = await Effect.runPromise(Effect.either(ro.delete("s1")));
      expect(Either.isLeft(deleteResult)).toBe(true);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("ClineStore + ClineRepository over a real sessions.db", () => {
  test("install writes artifacts and the index row; the repository reads it back", async () =>
    withTempDir(async (dir) => {
      const store = ClineStore.make(openSessionsDb, dir);
      const id = await Effect.runPromise(
        store.install(session, "cline-1").pipe(Effect.provide(fsLayer)),
      );
      expect(id).toBe("cline-1");

      const manifest = JSON.parse(
        readFileSync(join(dir, "sessions", "cline-1", "cline-1.json"), "utf8"),
      );
      expect(manifest.session_id).toBe("cline-1");
      expect(manifest.cwd).toBe("/work");

      const sqlite = openSessionsDb(join(dir, "db", "sessions.db"), true);
      try {
        const row = sqlite.get<{ status: string; pid: number }>(
          "select status, pid from sessions where session_id = ?",
          "cline-1",
        );
        expect(row).toEqual({ status: "completed", pid: 0 });
      } finally {
        sqlite.close();
      }

      const repo = ClineRepository.makeClineSessionRepository({ dataDir: dir });
      expect(await Effect.runPromise(repo.hasSession("cline-1"))).toBe(true);
      expect(await Effect.runPromise(repo.hasSession("missing"))).toBe(false);

      const listed = await Effect.runPromise(repo.list());
      expect(listed.map((s) => s.id)).toEqual(["cline-1"]);
      expect(listed[0]?.title).toBe("Bun storage round-trip");

      const found = await Effect.runPromise(repo.getById("cline-1"));
      expect(Option.isSome(found)).toBe(true);
      if (Option.isSome(found)) {
        expect(found.value.backendType).toBe("cline");
        expect(found.value.nodes.length).toBeGreaterThan(0);
      }
      expect(Option.isNone(await Effect.runPromise(repo.getById("missing")))).toBe(true);

      for (const write of [repo.save(session), repo.delete("cline-1")]) {
        const result = await Effect.runPromise(Effect.either(write));
        expect(Either.isLeft(result)).toBe(true);
        if (Either.isLeft(result)) {
          expect(result.left).toBeInstanceOf(StorageError);
        }
      }
    }));

  test("install refuses a live-owned row unless forced", async () =>
    withTempDir(async (dir) => {
      const store = ClineStore.make(openSessionsDb, dir);
      await Effect.runPromise(store.install(session, "cline-1").pipe(Effect.provide(fsLayer)));

      // Rewrite the index row as a live owner: running status + our own pid.
      const dbPath = join(dir, "db", "sessions.db");
      const sqlite = openSessionsDb(dbPath, false);
      try {
        const row = {
          ...ClineIndex.sessionRow(session, "cline-1", join(dir, "x.messages.json")),
          status: "running",
          pid: process.pid,
        };
        sqlite.insertSession(
          [...ClineIndex.SESSION_COLUMNS],
          ClineIndex.SESSION_COLUMNS.map((column) => row[column]),
        );
      } finally {
        sqlite.close();
      }

      await expect(
        Effect.runPromise(store.install(session, "cline-1").pipe(Effect.provide(fsLayer))),
      ).rejects.toThrow(/live owner/);

      const forced = await Effect.runPromise(
        store.install(session, "cline-1", { force: true }).pipe(Effect.provide(fsLayer)),
      );
      expect(forced).toBe("cline-1");
    }));
});
