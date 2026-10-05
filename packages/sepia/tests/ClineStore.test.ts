/**
 * `ClineStore` under node: the sqlite driver is an injected `openDb` seam,
 * so a scriptable fake covers install/indexRow without `bun:sqlite` (the
 * real driver is exercised by `tests/storage.bun.test.ts` under `bun test`).
 */
import * as FileSystem from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Effect, Layer, Option } from "effect";
import { expect, test } from "vite-plus/test";
import * as ClineStore from "../src/ClineStore.js";
import { SESSIONS_DDL } from "../src/ClineIndex.js";
import { MessageNode, Session } from "../src/Domain.js";
import type { SessionSqlite } from "../src/SessionSqlite.js";

const session = (over: Partial<Parameters<typeof Session.make>[0]> = {}): Session =>
  Session.make({
    id: "devin-1",
    title: "t",
    workingDirectory: "/work",
    model: "m",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_005,
    mainChainId: 0,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "hi",
        createdAt: 1,
        metadata: null,
      }),
    ],
    ...over,
  });

/** A fake `sessions.db`: one sessions table, rows keyed by session_id. */
const fakeDb = (
  rows: Record<string, { status: string; pid: number }> = {},
  opts: { hasTable?: boolean; failOnOpen?: boolean; failOnWriteOpen?: boolean } = {},
) => {
  const state = { rows, inserted: [] as Array<ReadonlyArray<unknown>>, closed: 0, ranDdl: false };
  const openDb: ClineStore.OpenSessionSqlite = (_dbPath, readonly) => {
    if (opts.failOnOpen === true || (opts.failOnWriteOpen === true && !readonly))
      throw new Error("db is corrupt");
    return {
      run: (sql) => {
        if (sql === SESSIONS_DDL) state.ranDdl = true;
      },
      get: <T>(_sql: string, ...params: Array<string>): T | null =>
        (state.rows[params[0]!] ?? null) as T | null,
      allTables: () => (opts.hasTable === false ? [] : ["sessions"]),
      insertSession: (_columns, values) => {
        state.inserted.push(values);
      },
      close: () => {
        state.closed += 1;
      },
    } satisfies SessionSqlite;
  };
  return { openDb, state };
};

/** In-memory fs where `dbExists` controls the index probe. */
const fsWith = (dbExists: boolean) => {
  const files = new Map<string, string>();
  const dirs: Array<string> = [];
  const layer = FileSystem.layerNoop({
    exists: (path) => Effect.succeed(path.endsWith("sessions.db") ? dbExists : files.has(path)),
    makeDirectory: (path) =>
      Effect.sync(() => {
        dirs.push(path);
      }),
    writeFileString: (path, data) =>
      Effect.sync(() => {
        files.set(path, data);
      }),
  });
  return { layer: Layer.merge(layer, Path.layer), files, dirs };
};

const install = (
  store: ClineStore.ClineStoreService,
  s: Session,
  layer: Layer.Layer<FileSystem.FileSystem | Path.Path>,
  sessionId?: string,
  options?: { readonly force?: boolean },
) => Effect.runPromise(store.install(s, sessionId, options).pipe(Effect.provide(layer)));

test("install writes artifacts, runs the DDL and registers the index row", async () => {
  const { openDb, state } = fakeDb();
  const { layer, files, dirs } = fsWith(false);
  const store = ClineStore.make(openDb, "/data");

  const id = await install(store, session(), layer, "sess-9");

  expect(id).toBe("sess-9");
  expect(dirs).toContain("/data/sessions/sess-9");
  expect(dirs).toContain("/data/db");
  const manifest = JSON.parse(files.get("/data/sessions/sess-9/sess-9.json") ?? "{}");
  expect(manifest.session_id).toBe("sess-9");
  const messages = JSON.parse(files.get("/data/sessions/sess-9/sess-9.messages.json") ?? "{}");
  expect(messages.sessionId).toBe("sess-9");
  expect(state.ranDdl).toBe(true);
  expect(state.inserted).toHaveLength(1);
  // the row was registered under the requested id, marked complete
  expect(state.inserted[0]).toContain("sess-9");
  expect(state.closed).toBe(1);
});

test("install mints a cline-style id when none is given", async () => {
  const { openDb } = fakeDb();
  const { layer, files } = fsWith(false);
  const store = ClineStore.make(openDb, "/data");
  const id = await install(store, session({ createdAt: 1_700_000_000 }), layer);
  expect(id).toMatch(/^1700000000000_[a-z0-9]{5}$/);
  expect(files.has(`/data/sessions/${id}/${id}.json`)).toBe(true);
});

test("install refuses a live-owned session unless forced", async () => {
  // `pid: process.pid` is alive; `status: running` makes the row live-owned.
  const { openDb } = fakeDb({ "sess-9": { status: "running", pid: process.pid } });
  const { layer } = fsWith(true);
  const store = ClineStore.make(openDb, "/data");

  await expect(install(store, session(), layer, "sess-9")).rejects.toThrow(
    "still belongs to a live owner",
  );
  await expect(install(store, session(), layer, "sess-9", { force: true })).resolves.toBe("sess-9");
});

test("install proceeds when the recorded owner is dead or the row is absent", async () => {
  const { openDb } = fakeDb({ "sess-1": { status: "running", pid: 2_000_000_000 } });
  const { layer } = fsWith(true);
  const store = ClineStore.make(openDb, "/data");
  await expect(install(store, session(), layer, "sess-1")).resolves.toBe("sess-1");
});

test("indexRow reports none for a missing db, no table or no row", async () => {
  const { openDb } = fakeDb();

  const missing = await Effect.runPromise(
    ClineStore.indexRow(openDb, { exists: () => Effect.succeed(false) } as never, "/db", "s"),
  );
  expect(Option.isNone(missing)).toBe(true);

  const { openDb: noTable } = fakeDb({}, { hasTable: false });
  const none = await Effect.runPromise(
    ClineStore.indexRow(noTable, { exists: () => Effect.succeed(true) } as never, "/db", "s"),
  );
  expect(Option.isNone(none)).toBe(true);

  const row = await Effect.runPromise(
    ClineStore.indexRow(
      fakeDb({ s: { status: "completed", pid: 0 } }).openDb,
      { exists: () => Effect.succeed(true) } as never,
      "/db",
      "s",
    ),
  );
  expect(Option.getOrUndefined(row)).toEqual({ status: "completed", pid: 0 });
});

test("indexRow wraps a driver failure as a ConversionError", async () => {
  const { openDb } = fakeDb({}, { failOnOpen: true });
  await expect(
    Effect.runPromise(
      ClineStore.indexRow(openDb, { exists: () => Effect.succeed(true) } as never, "/db", "s"),
    ),
  ).rejects.toThrow("Failed to read the Cline session index");

  // exists() itself failing maps to the same error shape
  await expect(
    Effect.runPromise(
      ClineStore.indexRow(
        openDb,
        { exists: () => Effect.fail(new Error("io")) } as never,
        "/db",
        "s",
      ),
    ),
  ).rejects.toThrow("Failed to read the Cline session index");
});

test("install wraps a register failure as a ConversionError", async () => {
  // the read-only indexRow probe succeeds; the writable register open fails
  const { openDb } = fakeDb({}, { failOnWriteOpen: true });
  const { layer } = fsWith(true);
  const store = ClineStore.make(openDb, "/data");
  await expect(install(store, session(), layer, "sess-9")).rejects.toThrow(
    "Failed to register the session",
  );
});
