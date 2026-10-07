/**
 * `ClineRepository` against a real temp data dir (the repository provides
 * its own BunFileSystem/Path layer — the same convention as the ClaudeCode
 * repository tests). Covers the read-only `SessionRepository` surface and
 * the `truncateClineSession` refusal paths the rewind tests do not reach.
 */
import { Effect, Either, Option } from "effect";
import { expect, test } from "vite-plus/test";
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as ClineRepository from "../src/ClineRepository.js";
import type { MessageNode } from "sepia-core";

const withTempDir = async <T>(prefix: string, fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const writeClineSession = (
  dataDir: string,
  id: string,
  manifest: Record<string, unknown> = {},
  messages: Record<string, unknown> = {},
): void => {
  const dir = join(dataDir, "sessions", id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, `${id}.messages.json`),
    JSON.stringify({
      version: 1,
      sessionId: id,
      messages: [
        { id: "m0", role: "user", content: [{ type: "text", text: "first" }], ts: 1000 },
        { id: "m1", role: "assistant", content: [{ type: "text", text: "answer" }], ts: 2000 },
      ],
      ...messages,
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
      model: "cline-pass/glm-5-2",
      ...manifest,
    }),
  );
};

const runEither = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(Effect.either(effect));

test("list summarizes manifests, skipping non-sessions and unparseable ones", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    writeClineSession(dataDir, "s1", { metadata: { title: "  Titled session  " } });
    writeClineSession(dataDir, "s2", { prompt: "<task>do <b>things</b></task>" });
    // a subagent id pattern lands its lineage on the summary
    writeClineSession(dataDir, "s1__agent_9");
    // a dir whose only json is the messages file → not a manifest
    mkdirSync(join(dataDir, "sessions", "msg-only"), { recursive: true });
    writeFileSync(join(dataDir, "sessions", "msg-only", "m.messages.json"), "{}");
    // an unparseable manifest is skipped, not fatal
    writeClineSession(dataDir, "broken");
    writeFileSync(join(dataDir, "sessions", "broken", "broken.json"), "{not json");
    // a stray file at the sessions root is ignored
    writeFileSync(join(dataDir, "sessions", "loose.txt"), "x");

    const repo = ClineRepository.makeClineSessionRepository({ dataDir });
    const sessions = await Effect.runPromise(repo.list());
    const byId = new Map(sessions.map((s) => [s.id, s]));

    expect(byId.get("s1")?.title).toBe("Titled session");
    expect(byId.get("s1")?.model).toBe("glm-5-2"); // cline-pass/ prefix stripped
    expect(byId.get("s1")?.backendType).toBe("cline");
    expect(byId.get("s1")?.createdAt).toBe(
      Math.floor(new Date("2026-01-01T00:00:00Z").getTime() / 1000),
    );
    expect(byId.get("s2")?.title).toBe("do things");
    expect(Option.getOrUndefined(byId.get("s1__agent_9")!.parentSessionId)).toBe("s1");
    expect(Option.getOrUndefined(byId.get("s1__agent_9")!.agentId)).toBe("agent_9");
    expect(byId.has("msg-only")).toBe(false);
    expect(byId.has("broken")).toBe(false);
  }));

test("list treats a missing sessions dir as empty and a file root as an error", async () => {
  const empty = ClineRepository.makeClineSessionRepository({
    dataDir: join(tmpdir(), "sepia-cline-repo-missing"),
  });
  expect(await Effect.runPromise(empty.list())).toEqual([]);
  expect(await Effect.runPromise(empty.hasSession("x"))).toBe(false);

  // `sessions` exists but is a file — exists() is true, readDirectory fails
  const dataDir = mkdtempSync(join(tmpdir(), "sepia-cline-repo-"));
  writeFileSync(join(dataDir, "sessions"), "not a dir");
  const repo = ClineRepository.makeClineSessionRepository({ dataDir });
  await expect(Effect.runPromise(repo.list())).rejects.toThrow("Failed to list cline sessions");
});

test("list caches manifest reads until a manifest or the dir set changes", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    writeClineSession(dataDir, "s1", { metadata: { title: "One" } });
    // a dangling-symlink entry at the sessions root — unstatable, skipped
    symlinkSync(join(dataDir, "sessions", "nonexistent"), join(dataDir, "sessions", "ghost"));
    const repo = ClineRepository.makeClineSessionRepository({ dataDir });

    const first = await Effect.runPromise(repo.list());
    expect(first[0]?.title).toBe("One");
    expect(first.map((s) => s.id)).toEqual(["s1"]);
    // unchanged stamp → the cached array comes back verbatim
    expect(await Effect.runPromise(repo.list())).toBe(first);

    // editing the manifest bumps its mtime → re-read
    const manifest = join(dataDir, "sessions", "s1", "s1.json");
    writeFileSync(
      manifest,
      JSON.stringify({ session_id: "s1", cwd: "/work", metadata: { title: "Two" } }),
    );
    utimesSync(manifest, new Date(), new Date(Date.now() + 10_000));
    const third = await Effect.runPromise(repo.list());
    expect(third).not.toBe(first);
    expect(third[0]?.title).toBe("Two");

    // a new session dir busts via the listing stamp
    writeClineSession(dataDir, "s2");
    expect((await Effect.runPromise(repo.list())).map((s) => s.id).sort()).toEqual(["s1", "s2"]);
  }));

test("getById loads the transcript; unknown and unreadable ids degrade correctly", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    writeClineSession(dataDir, "s1");
    // a dir with a manifest but no transcript — fromDirectory fails reading it
    const dir = join(dataDir, "sessions", "no-messages");
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, "no-messages.json"),
      JSON.stringify({ session_id: "no-messages", cwd: "/w" }),
    );

    const repo = ClineRepository.makeClineSessionRepository({ dataDir });
    const found = await Effect.runPromise(repo.getById("s1"));
    expect(Option.isSome(found)).toBe(true);
    expect(Option.getOrThrow(found).nodes.length).toBeGreaterThan(0);
    expect(Option.getOrThrow(found).backendType).toBe("cline");

    expect(Option.isNone(await Effect.runPromise(repo.getById("ghost")))).toBe(true);
    await expect(Effect.runPromise(repo.getById("no-messages"))).rejects.toThrow();
  }));

test("hasSession reflects the sessions dir; save/delete are read-only", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    writeClineSession(dataDir, "s1");
    const repo = ClineRepository.makeClineSessionRepository({ dataDir });
    expect(await Effect.runPromise(repo.hasSession("s1"))).toBe(true);
    expect(await Effect.runPromise(repo.hasSession("nope"))).toBe(false);
    await expect(Effect.runPromise(repo.save(null as never))).rejects.toThrow("read-only");
    await expect(Effect.runPromise(repo.delete("s1"))).rejects.toThrow("read-only");
  }));

/* ---- truncateClineSession refusals ------------------------------------ */

const truncated = (
  dataDir: string,
  id: string,
  kept: Array<MessageNode>,
  removed: Array<MessageNode>,
) => runEither(ClineRepository.truncateClineSession({ dataDir }, id, kept, removed));

const taggedNode = (nodeId: number, clineMessageIndex?: number): MessageNode =>
  ({ nodeId, metadata: { clineMessageIndex } }) as MessageNode;

test("truncateClineSession refuses a dir with no manifest", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    const dir = join(dataDir, "sessions", "s1");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "s1.messages.json"), "{}");
    const result = await truncated(dataDir, "s1", [], []);
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("No session metadata json");
  }));

test("truncateClineSession refuses a manifest that is not JSON", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    const dir = join(dataDir, "sessions", "s1");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "s1.json"), "{not json");
    const result = await truncated(dataDir, "s1", [], []);
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("not valid JSON");
  }));

test("truncateClineSession refuses an unparseable or message-less transcript", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    const dir = join(dataDir, "sessions", "s1");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "s1.json"), JSON.stringify({ session_id: "s1" }));
    writeFileSync(join(dir, "s1.messages.json"), "{oops");
    let result = await truncated(dataDir, "s1", [], []);
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("not valid JSON");

    writeFileSync(join(dir, "s1.messages.json"), JSON.stringify({ notMessages: true }));
    result = await truncated(dataDir, "s1", [], []);
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("no message array");
  }));

test("truncateClineSession refuses when it cannot prove the transcript is the session's", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    // no sessionId on the data, and the file is not named `<id>.messages.json`
    const dir = join(dataDir, "sessions", "s1");
    mkdirSync(dir, { recursive: true });
    const other = join(dataDir, "sessions", "s1", "shared.messages.json");
    writeFileSync(other, JSON.stringify({ messages: [] }));
    writeFileSync(join(dir, "s1.json"), JSON.stringify({ session_id: "s1", messages_path: other }));
    const result = await truncated(dataDir, "s1", [], []);
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("Cannot prove");
  }));

test("truncateClineSession refuses a removed node with no transcript index", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    writeClineSession(dataDir, "s1");
    const result = await truncated(
      dataDir,
      "s1",
      [],
      [{ nodeId: 7, metadata: null } as MessageNode],
    );
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result))
      expect(result.left.message).toContain("no recorded transcript entry");
  }));

test("truncateClineSession is a no-op when the kept set already reaches the tail", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    writeClineSession(dataDir, "s1");
    const result = await Effect.runPromise(
      ClineRepository.truncateClineSession({ dataDir }, "s1", [taggedNode(0, 99)], []),
    );
    expect(result.removedMessages).toBe(0);
  }));

test("list skips a session dir whose contents or manifest are unreadable", async () =>
  withTempDir("sepia-cline-repo-", async (dataDir) => {
    const locked = join(dataDir, "sessions", "locked");
    mkdirSync(locked, { recursive: true });
    chmodSync(locked, 0o000);

    const halfRead = join(dataDir, "sessions", "half");
    mkdirSync(halfRead);
    const manifest = join(halfRead, "half.json");
    writeFileSync(manifest, JSON.stringify({ session_id: "half" }));
    chmodSync(manifest, 0o000);

    const repo = ClineRepository.makeClineSessionRepository({ dataDir });
    try {
      expect(await Effect.runPromise(repo.list())).toEqual([]);
    } finally {
      chmodSync(locked, 0o755);
      chmodSync(manifest, 0o644);
    }
  }));
