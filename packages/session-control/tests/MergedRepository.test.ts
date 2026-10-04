import { describe, expect, it } from "vite-plus/test";
import { Effect, Option } from "effect";
import { Session, SessionRepository } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";
import { agentForBackend, mergeRepositories } from "../src/MergedRepository.js";

const session = (id: string, lastActivityAt: number, backendType = "windsurf"): Session =>
  new Session({
    id,
    title: `Session ${id}`,
    workingDirectory: "/work",
    backendType,
    model: "test-model",
    createdAt: 0,
    lastActivityAt,
    mainChainId: 0,
    metadata: null,
    nodes: [],
  });

const repo = (
  sessions: ReadonlyArray<Session>,
  fail?: { list?: boolean; getById?: boolean; hasSession?: boolean },
): SessionRepositoryService =>
  SessionRepository.of({
    save: () => Effect.void,
    getById: (id) =>
      fail?.getById === true
        ? Effect.fail(new Error("getById down") as never)
        : Effect.succeed(Option.fromNullable(sessions.find((s) => s.id === id))),
    list: () =>
      fail?.list === true ? Effect.fail(new Error("list down") as never) : Effect.succeed(sessions),
    delete: () => Effect.void,
    hasSession: (id) =>
      fail?.hasSession === true
        ? Effect.fail(new Error("hasSession down") as never)
        : Effect.succeed(sessions.some((s) => s.id === id)),
  });

const run = <A, E>(effect: Effect.Effect<A, E>): Promise<A> => Effect.runPromise(effect);

describe("agentForBackend", () => {
  it("maps cline backends to cline, everything else to devin", () => {
    expect(agentForBackend("cline")).toBe("cline");
    expect(agentForBackend("windsurf")).toBe("devin");
    expect(agentForBackend("anything")).toBe("devin");
  });
});

describe("mergeRepositories.list", () => {
  it("merges primary and extras, deduped by id, sorted by activity desc", async () => {
    const merged = mergeRepositories(repo([session("a", 100), session("b", 300)]), [
      repo([session("b", 50), session("c", 200)]),
    ]);
    const list = await run(merged.list());
    // "b" collides — the primary's copy wins; order is most-recent-first.
    expect(list.map((s) => `${s.id}:${s.lastActivityAt}`)).toEqual(["b:300", "c:200", "a:100"]);
  });

  it("an extra repo failure degrades to empty rather than failing the list", async () => {
    const merged = mergeRepositories(repo([session("a", 1)]), [
      repo([], { list: true }),
      repo([session("b", 2)]),
    ]);
    expect((await run(merged.list())).map((s) => s.id)).toEqual(["b", "a"]);
  });

  it("a primary failure propagates", async () => {
    const merged = mergeRepositories(repo([], { list: true }), [repo([session("b", 1)])]);
    await expect(run(merged.list())).rejects.toThrow("list down");
  });
});

describe("mergeRepositories.getById", () => {
  const clineSession = session("shared", 1, "cline");
  const devinSession = session("shared", 1, "windsurf");

  it("returns the primary's copy when it exists and the agent matches", async () => {
    const merged = mergeRepositories(repo([devinSession]), [repo([clineSession])]);
    const found = await run(merged.getById("shared"));
    expect(Option.isSome(found) && found.value.backendType).toBe("windsurf");
  });

  it("scopes the primary hit by agentId — wrong agent falls through to extras", async () => {
    const merged = mergeRepositories(repo([devinSession]), [repo([clineSession])]);
    const found = await run(merged.getById("shared", "cline"));
    expect(Option.isSome(found) && found.value.backendType).toBe("cline");
  });

  it("returns none when no repo has a matching session", async () => {
    const merged = mergeRepositories(repo([devinSession]), [repo([])]);
    expect(await run(merged.getById("ghost"))).toEqual(Option.none());
    // Primary has it but under the wrong backend for the requested agent.
    expect(await run(merged.getById("shared", "cline"))).toEqual(Option.none());
  });

  it("an extra repo failure degrades to not-found", async () => {
    const merged = mergeRepositories(repo([]), [repo([], { getById: true })]);
    expect(await run(merged.getById("x"))).toEqual(Option.none());
  });

  it("a primary failure propagates", async () => {
    const merged = mergeRepositories(repo([], { getById: true }), []);
    await expect(run(merged.getById("x"))).rejects.toThrow("getById down");
  });
});

describe("mergeRepositories.hasSession", () => {
  it("is true when any repo has the session, tolerating extra failures", async () => {
    const merged = mergeRepositories(repo([session("a", 1)]), [
      repo([], { hasSession: true }),
      repo([session("b", 1)]),
    ]);
    expect(await run(merged.hasSession("a"))).toBe(true);
    expect(await run(merged.hasSession("b"))).toBe(true);
    expect(await run(merged.hasSession("z"))).toBe(false);
  });

  it("a primary failure propagates", async () => {
    const merged = mergeRepositories(repo([], { hasSession: true }), []);
    await expect(run(merged.hasSession("a"))).rejects.toThrow("hasSession down");
  });
});

describe("mergeRepositories writes", () => {
  it("delegates save and delete to the primary only", async () => {
    const calls: string[] = [];
    const primary: SessionRepositoryService = SessionRepository.of({
      save: (item) =>
        Effect.sync(() => {
          calls.push(`save:${item.id}`);
        }),
      getById: () => Effect.succeed(Option.none()),
      list: () => Effect.succeed([]),
      delete: (id) =>
        Effect.sync(() => {
          calls.push(`delete:${id}`);
        }),
      hasSession: () => Effect.succeed(false),
    });
    const extra: SessionRepositoryService = SessionRepository.of({
      save: () =>
        Effect.sync(() => {
          calls.push("extra-save");
        }),
      getById: () => Effect.succeed(Option.none()),
      list: () => Effect.succeed([]),
      delete: () =>
        Effect.sync(() => {
          calls.push("extra-delete");
        }),
      hasSession: () => Effect.succeed(false),
    });
    const merged = mergeRepositories(primary, [extra]);
    await run(merged.save(session("w", 1)));
    await run(merged.delete("w"));
    expect(calls).toEqual(["save:w", "delete:w"]);
  });
});
