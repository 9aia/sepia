/**
 * Effect-level coverage for `Conversion.ts`: `importSession`/`importCline`/
 * `exportCline`/`installCline`/`listSessions` against injected
 * `SessionRepository`/`ClineStore` fakes (plus a real temp dir where the
 * Cline writers need a filesystem), and the schema-default branches of
 * `sessionFromJson`/`sessionFromHistory` the round-trip tests skip.
 */
import { describe, expect, it, vi } from "vite-plus/test";
import { Effect, Layer, Option } from "effect";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as Conversion from "../src/Conversion.js";
import { ClineStore } from "../src/ClineStore.js";
import { ConversionError, MessageNode, Session, StorageError } from "sepia-core";
import { SessionRepository, type SessionRepositoryService } from "sepia-core";

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const baseSession = (over: Partial<Parameters<typeof Session.make>[0]> = {}): Session =>
  Session.make({
    id: "sess-1",
    title: "t",
    workingDirectory: "/work",
    model: "m-1",
    createdAt: 1,
    lastActivityAt: 2,
    mainChainId: 0,
    cogsJson: "[]",
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

const repoWith = (impl: Partial<SessionRepositoryService>) =>
  Layer.succeed(SessionRepository, {
    save: () => Effect.void,
    getById: () => Effect.succeed(Option.none()),
    list: () => Effect.succeed([]),
    delete: () => Effect.void,
    hasSession: () => Effect.succeed(false),
    ...impl,
  });

const cogsWithModel = (model: string) =>
  JSON.stringify([{ lifetime: { Unique: "core/model" }, model }]);

describe("importSession", () => {
  it("leaves an already-imported session untouched", async () => {
    const saved: Array<Session> = [];
    const layer = repoWith({
      hasSession: () => Effect.succeed(true),
      save: (s) => Effect.sync(() => saved.push(s)),
    });
    const id = await Effect.runPromise(
      Conversion.importSession(baseSession()).pipe(Effect.provide(layer)),
    );
    expect(id).toBe("sess-1");
    expect(saved).toEqual([]);
  });

  it("grafts cogs from a same-cwd donor session", async () => {
    const saved: Array<Session> = [];
    const donor = baseSession({
      id: "donor",
      workingDirectory: "/work",
      cogsJson: cogsWithModel("swe-3"),
    });
    const layer = repoWith({
      list: () =>
        Effect.succeed([donor, baseSession({ id: "other", workingDirectory: "/elsewhere" })]),
      save: (s) => Effect.sync(() => saved.push(s)),
    });
    await Effect.runPromise(
      Conversion.importSession(baseSession({ cogsJson: "[]" })).pipe(Effect.provide(layer)),
    );
    expect(saved).toHaveLength(1);
    expect(saved[0]!.cogsJson).toBe(cogsWithModel("swe-3"));
  });

  it("falls back to a different-cwd donor, then to a model fill", async () => {
    const saved: Array<Session> = [];
    const donor = baseSession({
      id: "donor",
      workingDirectory: "/elsewhere",
      cogsJson: cogsWithModel("swe-3"),
    });
    const layer = repoWith({
      // the session itself plus a cog-less sibling are skipped as donors
      list: () =>
        Effect.succeed([
          baseSession({ id: "sess-1" }),
          baseSession({ id: "nogoods", cogsJson: "not json" }),
          donor,
        ]),
      save: (s) => Effect.sync(() => saved.push(s)),
    });
    await Effect.runPromise(
      Conversion.importSession(baseSession({ workingDirectory: "/work" })).pipe(
        Effect.provide(layer),
      ),
    );
    expect(saved[0]!.cogsJson).toBe(cogsWithModel("swe-3"));
  });

  it("fills the stub model cog when no donor exists — even when list fails", async () => {
    const saved: Array<Session> = [];
    const layer = repoWith({
      list: () => Effect.fail(new StorageError({ message: "db down" })),
      save: (s) => Effect.sync(() => saved.push(s)),
    });
    await Effect.runPromise(
      Conversion.importSession(
        baseSession({
          model: "m-9",
          cogsJson: JSON.stringify([{ lifetime: { Unique: "core/model" }, model: "" }]),
        }),
      ).pipe(Effect.provide(layer)),
    );
    // withModel rewrote the empty model cog with the session's model
    expect(JSON.parse(saved[0]!.cogsJson)[0].model).toBe("m-9");
  });

  it("passes the cogs blob through untouched when it is not cog JSON", async () => {
    const saved: Array<Session> = [];
    const layer = repoWith({
      list: () => Effect.succeed([]),
      save: (s) => Effect.sync(() => saved.push(s)),
    });
    for (const cogsJson of ["not json", JSON.stringify({ not: "a list" })]) {
      await Effect.runPromise(
        Conversion.importSession(baseSession({ cogsJson })).pipe(Effect.provide(layer)),
      );
    }
    expect(saved.map((s) => s.cogsJson)).toEqual(["not json", JSON.stringify({ not: "a list" })]);
  });
});

/* ---- cline helpers --------------------------------------------------- */

const writeClineDir = (dataDir: string, id: string): void => {
  const dir = join(dataDir, id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, `${id}.messages.json`),
    JSON.stringify({
      version: 1,
      sessionId: id,
      messages: [
        { id: "m0", role: "user", content: [{ type: "text", text: "hello" }], ts: 1000 },
        { id: "m1", role: "assistant", content: [{ type: "text", text: "hi" }], ts: 2000 },
      ],
    }),
  );
  writeFileSync(
    join(dir, `${id}.json`),
    JSON.stringify({
      version: 1,
      session_id: id,
      cwd: "/work",
      started_at: "2026-01-01T00:00:00.000Z",
      ended_at: "2026-01-01T00:00:05.000Z",
      status: "completed",
      model: "glm-5-2",
    }),
  );
};

describe("importCline", () => {
  it("imports a Cline dir into the store and skips existing/dry-run paths", async () => {
    const dataDir = mkdtempSync(join(tmpdir(), "sepia-cline-import-"));
    writeClineDir(dataDir, "c1");

    const saved: Array<Session> = [];
    const layer = Layer.mergeAll(
      repoWith({
        hasSession: () => Effect.succeed(false),
        list: () => Effect.succeed([]),
        save: (s) => Effect.sync(() => saved.push(s)),
      }),
      fsLayer,
    );
    const imported = await Effect.runPromise(
      Conversion.importCline(join(dataDir, "c1")).pipe(Effect.provide(layer)),
    );
    expect(imported).toBe("c1");
    expect(saved).toHaveLength(1);
    // fromDirectory marks the IR backend "windsurf"; the cline tag only
    // appears on repository reads.
    expect(saved[0]!.backendType).toBe("windsurf");

    // exists → the import short-circuits before touching the store
    const existsLayer = Layer.mergeAll(
      repoWith({ hasSession: () => Effect.succeed(true) }),
      fsLayer,
    );
    expect(
      await Effect.runPromise(
        Conversion.importCline(join(dataDir, "c1")).pipe(Effect.provide(existsLayer)),
      ),
    ).toBe("c1");

    // dryRun reports without saving
    const drySaved: Array<Session> = [];
    const dryLayer = Layer.mergeAll(
      repoWith({ save: (s) => Effect.sync(() => drySaved.push(s)) }),
      fsLayer,
    );
    await Effect.runPromise(
      Conversion.importCline(join(dataDir, "c1"), undefined, { dryRun: true }).pipe(
        Effect.provide(dryLayer),
      ),
    );
    expect(drySaved).toEqual([]);
  });

  it("fails on a missing Cline dir", async () => {
    const layer = Layer.mergeAll(repoWith({}), fsLayer);
    await expect(
      Effect.runPromise(
        Conversion.importCline(join(tmpdir(), "sepia-no-such-cline-dir")).pipe(
          Effect.provide(layer),
        ),
      ),
    ).rejects.toThrow("not found");
  });
});

describe("exportCline", () => {
  it("fails for an unknown session and writes files for a known one", async () => {
    const outDir = mkdtempSync(join(tmpdir(), "sepia-cline-export-"));
    const missing = Layer.mergeAll(repoWith({}), fsLayer);
    await expect(
      Effect.runPromise(Conversion.exportCline("ghost", outDir).pipe(Effect.provide(missing))),
    ).rejects.toThrow("Session not found: ghost");

    const layer = Layer.mergeAll(
      repoWith({ getById: () => Effect.succeed(Option.some(baseSession())) }),
      fsLayer,
    );
    await Effect.runPromise(Conversion.exportCline("sess-1", outDir).pipe(Effect.provide(layer)));
    expect(JSON.parse(readFileSync(join(outDir, "sess-1.json"), "utf8"))).toMatchObject({
      session_id: "sess-1",
    });

    // a second export without force reports "kept"
    const spy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await Effect.runPromise(Conversion.exportCline("sess-1", outDir).pipe(Effect.provide(layer)));
      expect(spy).toHaveBeenCalledWith(expect.stringContaining("leaving it untouched"));

      // dry-run reports without writing
      spy.mockClear();
      const dryDir = join(outDir, "dry");
      await Effect.runPromise(
        Conversion.exportCline("sess-1", dryDir, { dryRun: true }).pipe(Effect.provide(layer)),
      );
      expect(spy).toHaveBeenCalledWith(expect.stringContaining("Would export"));
    } finally {
      spy.mockRestore();
    }
  });
});

describe("installCline", () => {
  const clineStore = (impl: {
    install: (
      session: Session,
      sessionId?: string,
      options?: { readonly force?: boolean },
    ) => Effect.Effect<string, ConversionError>;
  }) => Layer.succeed(ClineStore, impl);

  it("fails for an unknown session", async () => {
    const layer = Layer.mergeAll(
      repoWith({}),
      clineStore({ install: () => Effect.succeed("x") }),
      fsLayer,
    );
    await expect(
      Effect.runPromise(Conversion.installCline("ghost", "/data").pipe(Effect.provide(layer))),
    ).rejects.toThrow("Session not found: ghost");
  });

  it("returns the id the store installed under", async () => {
    const seen: Array<{ session: Session; sessionId?: string; force?: boolean }> = [];
    const layer = Layer.mergeAll(
      repoWith({ getById: () => Effect.succeed(Option.some(baseSession())) }),
      fsLayer,
      clineStore({
        install: (session, sessionId, options) =>
          Effect.sync(() => {
            seen.push({ session, sessionId, force: options?.force });
            return sessionId ?? "minted-id";
          }),
      }),
    );
    const installed = await Effect.runPromise(
      Conversion.installCline("sess-1", "/data", "chosen-id", { force: true }).pipe(
        Effect.provide(layer),
      ),
    );
    expect(installed).toBe("chosen-id");
    expect(seen[0]).toMatchObject({ sessionId: "chosen-id", force: true });
  });

  it("wraps a store failure as a ConversionError", async () => {
    const layer = Layer.mergeAll(
      repoWith({ getById: () => Effect.succeed(Option.some(baseSession())) }),
      fsLayer,
      clineStore({
        install: () => Effect.fail(new ConversionError({ message: "live owner", cause: null })),
      }),
    );
    await expect(
      Effect.runPromise(Conversion.installCline("sess-1", "/data").pipe(Effect.provide(layer))),
    ).rejects.toThrow("live owner");
  });
});

describe("listSessions", () => {
  it("prints each session, or a placeholder for an empty store", async () => {
    const spy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await Effect.runPromise(
        Conversion.listSessions().pipe(
          Effect.provide(
            repoWith({
              list: () =>
                Effect.succeed([
                  baseSession({ id: "a", title: "first", workingDirectory: "/w1" }),
                  baseSession({ id: "b", title: "second", workingDirectory: "/w2" }),
                ]),
            }),
          ),
        ),
      );
      expect(spy).toHaveBeenCalledWith("a\tfirst\t/w1");
      expect(spy).toHaveBeenCalledWith("b\tsecond\t/w2");

      spy.mockClear();
      await Effect.runPromise(Conversion.listSessions().pipe(Effect.provide(repoWith({}))));
      expect(spy).toHaveBeenCalledWith("No sessions found");
    } finally {
      spy.mockRestore();
    }
  });
});

/* ---- wire-shape defaults --------------------------------------------- */

describe("sessionFromJson defaults", () => {
  it("fills every optional field of a minimal payload", () => {
    const decoded = Conversion.sessionFromJson({
      id: "min",
      title: "t",
      workingDirectory: "/w",
      model: "m",
      createdAt: 1,
      lastActivityAt: 2,
      mainChainId: 0,
      nodes: [
        {
          nodeId: 0,
          role: "user",
          content: "hi",
          createdAt: 1,
          toolCalls: [{ id: "c1", name: "read" }],
        },
        { nodeId: 1, role: "assistant", content: "ok", createdAt: 2 },
      ],
      promptHistory: [{ content: "hi", timestamp: 1 }],
    });
    expect(decoded.backendType).toBe("windsurf");
    expect(decoded.agentMode).toBe("accept-edits");
    expect(decoded.cogsJson).toBe("[]");
    expect(decoded.nodes[0]!.toolCalls[0]).toMatchObject({
      id: "c1",
      name: "read",
      index: 0,
      kind: "function",
      locations: [],
      diffs: [],
    });
    expect(Option.isNone(decoded.nodes[0]!.toolCallId)).toBe(true);
    expect(decoded.promptHistory[0]!.isShell).toBe(false);
  });

  it("rejects a payload that is not a session", () => {
    expect(() => Conversion.sessionFromJson({ nope: true })).toThrow();
    expect(() => Conversion.sessionFromJson("x")).toThrow();
  });
});

describe("sessionFromHistory edge fields", () => {
  const base = { id: "h", title: "t", cwd: "/w", model: "m" };

  it("drops unusable optional fields and stamps role metadata", () => {
    const session = Conversion.sessionFromHistory({
      ...base,
      history: [
        { role: "system", content: "prefix", createdAt: 0 },
        {
          role: "assistant",
          content: "a",
          createdAt: Number.NaN,
          toolName: "",
          thinking: "",
          thinkingSignature: 5 as unknown as string,
          model: 7 as unknown as string,
          toolStatus: "error",
          exitCode: 2,
          durationMs: 30,
        },
        { role: "tool", content: "out", createdAt: -5 },
        { role: "user", content: "when", createdAt: Number.NaN },
      ],
    });
    const [sys, assistant, tool] = session.nodes;
    // non-finite/absent timestamps fall back to the build time
    expect(assistant!.createdAt).toBe(session.createdAt);
    expect(tool!.createdAt).toBe(session.createdAt);
    expect((sys!.metadata as Record<string, unknown>).is_system_prefix).toBe(true);
    expect(Option.isNone(assistant!.toolName)).toBe(true);
    expect(Option.isNone(assistant!.thinking)).toBe(true);
    expect(Option.isNone(assistant!.thinkingSignature)).toBe(true);
    expect(Option.isNone(assistant!.model)).toBe(true);
    expect(Option.getOrUndefined(assistant!.toolResult)).toEqual({
      status: "error",
      exitCode: 2,
      durationMs: 30,
    });
    // prompt history falls back to now when createdAt is not finite
    expect(session.promptHistory).toHaveLength(1);
    expect(session.promptHistory[0]!.timestamp).toBe(session.createdAt * 1000);
  });
});
