/**
 * Store-level rewind coverage — the per-backend truncators behind
 * `POST /api/sessions/:id/rewind`:
 *   - `truncateClineSession` slices `<id>.messages.json` on the
 *     `clineMessageIndex` tags the reader stamps on each node;
 *   - `truncateClaudeTranscript` rewrites the `.jsonl` to end at the
 *     entry that produced the last kept node.
 * Both run against real temp dirs (same convention as the repository
 * tests — the writers provide their own BunFileSystem/Path layers).
 */
import { Effect, Either, Layer } from "effect";
import { expect, test } from "vite-plus/test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ClaudeCode } from "sepia-claude";
import { ClaudeCodeRepository } from "sepia-claude";
import { Cline } from "sepia-cline";
import { ClineRepository } from "sepia-cline";
import { Rewind } from "sepia-core";
import type { MessageNode, Session } from "sepia-core";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const withTempDir = async <T>(prefix: string, fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

/* ---- cline ----------------------------------------------------------- */

const clineMessages = {
  version: 1,
  updated_at: "2026-01-01T00:00:00.000Z",
  agent: "lead",
  sessionId: "sess-1",
  origin: { source: "cli", mode: "user", sessionId: "sess-1", version: "3.0.61" },
  // A field sepia never modeled — the truncation must preserve it verbatim.
  futureField: { nested: true },
  messages: [
    { id: "msg_0", role: "user", content: [{ type: "text", text: "first prompt" }], ts: 1000 },
    {
      id: "msg_1",
      role: "assistant",
      content: [{ type: "text", text: "first answer" }],
      ts: 2000,
    },
    { id: "msg_2", role: "user", content: [{ type: "text", text: "second prompt" }], ts: 3000 },
    {
      id: "msg_3",
      role: "assistant",
      content: [{ type: "text", text: "second answer" }],
      ts: 4000,
    },
  ],
};

const writeClineDir = (dataDir: string, id: string, messages: Record<string, unknown>): void => {
  const dir = join(dataDir, "sessions", id);
  mkdirSync(dir, { recursive: true });
  const messagesPath = join(dir, `${id}.messages.json`);
  writeFileSync(messagesPath, JSON.stringify(messages, null, 2));
  writeFileSync(
    join(dir, `${id}.json`),
    JSON.stringify(
      {
        version: 1,
        session_id: id,
        cwd: "/work",
        workspace_root: "/work",
        started_at: "2026-01-01T00:00:00.000Z",
        ended_at: "2026-01-01T00:00:05.000Z",
        status: "completed",
        model: "glm-5-2",
        messages_path: messagesPath,
      },
      null,
      2,
    ),
  );
};

const clineSessionAt = async (dataDir: string, id: string): Promise<Session> =>
  Effect.runPromise(
    Cline.fromDirectory(join(dataDir, "sessions", id)).pipe(Effect.provide(fsLayer)),
  );

const runEither = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(Effect.either(effect));

test("cline reader tags nodes with clineMessageIndex", async () =>
  withTempDir("sepia-cline-rewind-", async (dataDir) => {
    writeClineDir(dataDir, "sess-1", clineMessages);
    const session = await clineSessionAt(dataDir, "sess-1");
    const tagged = session.nodes.filter(
      (node) => (node.metadata as Record<string, unknown> | null)?.clineMessageIndex !== undefined,
    );
    expect(tagged.length).toBeGreaterThan(0);
    const secondUser = [...session.nodes].reverse().find((node) => node.role === "user");
    expect(secondUser).toBeDefined();
    expect((secondUser!.metadata as Record<string, unknown>).clineMessageIndex).toBe(2);
  }));

test("truncateClineSession slices the messages array and preserves other fields", async () =>
  withTempDir("sepia-cline-rewind-", async (dataDir) => {
    writeClineDir(dataDir, "sess-1", clineMessages);
    const session = await clineSessionAt(dataDir, "sess-1");
    const secondUser = [...session.nodes].reverse().find((node) => node.role === "user");
    const planned = Rewind.planRewind(session, { nodeId: secondUser!.nodeId });
    expect(planned.ok).toBe(true);
    if (!planned.ok) return;

    const result = await Effect.runPromise(
      ClineRepository.truncateClineSession(
        { dataDir },
        "sess-1",
        planned.plan.kept,
        planned.plan.removed,
      ),
    );
    expect(result.removedMessages).toBe(1);

    const written = JSON.parse(
      readFileSync(join(dataDir, "sessions", "sess-1", "sess-1.messages.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(written.futureField).toEqual({ nested: true });
    expect((written.messages as ReadonlyArray<{ id: string }>).map((m) => m.id)).toEqual([
      "msg_0",
      "msg_1",
      "msg_2",
    ]);
    // The manifest is untouched.
    const manifest = JSON.parse(
      readFileSync(join(dataDir, "sessions", "sess-1", "sess-1.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(manifest.ended_at).toBe("2026-01-01T00:00:05.000Z");

    // The re-read session ends at the second user turn.
    const reread = await clineSessionAt(dataDir, "sess-1");
    expect(reread.nodes[reread.nodes.length - 1]!.role).toBe("user");
    expect(reread.nodes[reread.nodes.length - 1]!.content).toBe("second prompt");
  }));

test("truncateClineSession refuses a transcript owned by another session", async () =>
  withTempDir("sepia-cline-rewind-", async (dataDir) => {
    // A subagent manifest pointing into the parent's transcript must not
    // truncate the parent's history.
    writeClineDir(dataDir, "sub", {
      ...clineMessages,
      sessionId: "parent",
      origin: { sessionId: "parent" },
    });
    const result = await runEither(
      ClineRepository.truncateClineSession(
        { dataDir },
        "sub",
        [],
        [{ metadata: { clineMessageIndex: 0 } } as MessageNode],
      ),
    );
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("belongs to parent");
  }));

test("truncateClineSession refuses a messages_path outside the data dir", async () =>
  withTempDir("sepia-cline-rewind-", async (dataDir) => {
    const dir = join(dataDir, "sessions", "sess-1");
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, "sess-1.json"),
      JSON.stringify({ session_id: "sess-1", messages_path: "/etc/passwd" }),
    );
    const result = await runEither(
      ClineRepository.truncateClineSession(
        { dataDir },
        "sess-1",
        [],
        [{ metadata: { clineMessageIndex: 0 } } as MessageNode],
      ),
    );
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("escapes the data dir");
  }));

test("truncateClineSession fails when the session dir is missing", async () =>
  withTempDir("sepia-cline-rewind-", async (dataDir) => {
    const result = await runEither(
      ClineRepository.truncateClineSession({ dataDir }, "ghost", [], []),
    );
    expect(Either.isLeft(result)).toBe(true);
  }));

/* ---- claude ---------------------------------------------------------- */

const claudeLine = (entry: Record<string, unknown>): string => JSON.stringify(entry);

const claudeTranscript = [
  claudeLine({ type: "summary", summary: "Rewind me", leafUuid: "u1" }),
  claudeLine({
    type: "user",
    uuid: "u1",
    parentUuid: null,
    sessionId: "sess-1",
    cwd: "/work/proj",
    timestamp: "2026-01-01T00:00:01.000Z",
    message: { role: "user", content: "first prompt" },
  }),
  claudeLine({
    type: "assistant",
    uuid: "u2",
    parentUuid: "u1",
    sessionId: "sess-1",
    timestamp: "2026-01-01T00:00:02.000Z",
    message: { role: "assistant", content: [{ type: "text", text: "first answer" }] },
  }),
  claudeLine({
    type: "file-history-snapshot",
    messageId: "snap-1",
    snapshot: {
      messageId: "snap-1",
      timestamp: "2026-01-01T00:00:02.500Z",
      trackedFileBackups: {},
    },
  }),
  claudeLine({
    type: "user",
    uuid: "u3",
    parentUuid: "u2",
    sessionId: "sess-1",
    timestamp: "2026-01-01T00:00:03.000Z",
    message: { role: "user", content: "second prompt" },
  }),
  claudeLine({
    type: "assistant",
    uuid: "u4",
    parentUuid: "u3",
    sessionId: "sess-1",
    timestamp: "2026-01-01T00:00:04.000Z",
    message: { role: "assistant", content: [{ type: "text", text: "second answer" }] },
  }),
].join("\n");

test("truncateClaudeTranscript rewrites the file to end at the kept node's entry", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    mkdirSync(join(projectsDir, "-work-proj"), { recursive: true });
    const filePath = join(projectsDir, "-work-proj", "sess-1.jsonl");
    writeFileSync(filePath, claudeTranscript + "\n");

    const session = await Effect.runPromise(
      ClaudeCode.fromFile(filePath).pipe(Effect.provide(fsLayer)),
    );
    // u2 = the first assistant answer; u3/u4 nodes get dropped.
    const target = session.nodes.find((node) => node.content === "first answer");
    expect(target).toBeDefined();
    const planned = Rewind.planRewind(session, { nodeId: target!.nodeId });
    expect(planned.ok).toBe(true);
    if (!planned.ok) return;

    const result = await Effect.runPromise(
      ClaudeCodeRepository.truncateClaudeTranscript(
        { projectsDir },
        "sess-1",
        planned.plan.kept,
        planned.plan.removed,
      ),
    );
    expect(result.removedEntries).toBe(3);

    const lines = readFileSync(filePath, "utf8").trim().split("\n");
    // Kept lines are byte-for-byte identical — nothing re-serialized.
    expect(lines.join("\n") + "\n").toBe(
      claudeTranscript.split("\n").slice(0, 3).join("\n") + "\n",
    );

    const reread = await Effect.runPromise(
      ClaudeCode.fromFile(filePath).pipe(Effect.provide(fsLayer)),
    );
    expect(reread.nodes[reread.nodes.length - 1]!.content).toBe("first answer");
  }));

test("truncateClaudeTranscript preserves non-JSON plumbing lines and a missing trailing newline", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    mkdirSync(join(projectsDir, "-work-proj"), { recursive: true });
    const filePath = join(projectsDir, "-work-proj", "sess-1.jsonl");
    // A torn line the writer can't parse still rides the cut.
    const raw = "not-json\n" + claudeTranscript;
    writeFileSync(filePath, raw);

    const session = await Effect.runPromise(
      ClaudeCode.fromFile(filePath).pipe(Effect.provide(fsLayer)),
    );
    const target = session.nodes.find((node) => node.content === "first answer");
    const planned = Rewind.planRewind(session, { nodeId: target!.nodeId });
    if (!planned.ok) throw new Error("plan failed");

    const result = await Effect.runPromise(
      ClaudeCodeRepository.truncateClaudeTranscript(
        { projectsDir },
        "sess-1",
        planned.plan.kept,
        planned.plan.removed,
      ),
    );
    expect(result.removedEntries).toBe(3);
    // No trailing newline added — the file keeps the form it had.
    expect(readFileSync(filePath, "utf8").endsWith("\n")).toBe(false);
    expect(readFileSync(filePath, "utf8").split("\n")[0]).toBe("not-json");
  }));

test("truncateClaudeTranscript refuses a cut the file's order can't honor", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    mkdirSync(join(projectsDir, "-work-proj"), { recursive: true });
    const filePath = join(projectsDir, "-work-proj", "sess-1.jsonl");
    writeFileSync(filePath, claudeTranscript + "\n");

    // Kept node sits at the last line while a removed node sits earlier —
    // no real plan produces this, and the writer must not silently keep it.
    const kept = [{ nodeId: 1, metadata: { uuid: "u4" } } as MessageNode];
    const removed = [{ nodeId: 2, metadata: { uuid: "u1" } } as MessageNode];
    const result = await runEither(
      ClaudeCodeRepository.truncateClaudeTranscript({ projectsDir }, "sess-1", kept, removed),
    );
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("cannot honor this cut");
  }));

test("truncateClaudeTranscript is a no-op when the cut is already the tail", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    mkdirSync(join(projectsDir, "-work-proj"), { recursive: true });
    const filePath = join(projectsDir, "-work-proj", "sess-1.jsonl");
    const before = claudeTranscript + "\n";
    writeFileSync(filePath, before);

    const result = await Effect.runPromise(
      ClaudeCodeRepository.truncateClaudeTranscript(
        { projectsDir },
        "sess-1",
        [{ nodeId: 1, metadata: { uuid: "u4" } } as MessageNode],
        [],
      ),
    );
    expect(result.removedEntries).toBe(0);
    expect(readFileSync(filePath, "utf8")).toBe(before);
  }));

test("truncateClaudeTranscript refuses an unknown transcript", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const result = await runEither(
      ClaudeCodeRepository.truncateClaudeTranscript(
        { projectsDir: join(root, "projects") },
        "ghost",
        [],
        [],
      ),
    );
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("not found");
  }));

test("truncateClaudeTranscript fails on a node with no locatable entry", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    mkdirSync(join(projectsDir, "-work-proj"), { recursive: true });
    writeFileSync(join(projectsDir, "-work-proj", "sess-1.jsonl"), claudeTranscript + "\n");

    for (const node of [
      { nodeId: 99, metadata: { uuid: "no-such-uuid" } },
      { nodeId: 98, metadata: { uuid: "" } },
      { nodeId: 97, metadata: null },
    ] as ReadonlyArray<unknown> as ReadonlyArray<MessageNode>) {
      const result = await runEither(
        ClaudeCodeRepository.truncateClaudeTranscript({ projectsDir }, "sess-1", [], [node]),
      );
      expect(Either.isLeft(result)).toBe(true);
      if (Either.isLeft(result))
        expect(result.left.message).toContain("no locatable transcript entry");
    }
  }));

test("truncateClaudeTranscript with an empty kept set clears the transcript", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    mkdirSync(join(projectsDir, "-work-proj"), { recursive: true });
    const filePath = join(projectsDir, "-work-proj", "sess-1.jsonl");
    writeFileSync(filePath, claudeTranscript + "\n");

    // A checkpoint older than every node produces this shape — nothing to
    // keep, so the whole file goes.
    const result = await Effect.runPromise(
      ClaudeCodeRepository.truncateClaudeTranscript(
        { projectsDir },
        "sess-1",
        [],
        [{ nodeId: 1, metadata: { uuid: "u1" } } as MessageNode],
      ),
    );
    expect(result.removedEntries).toBe(6);
    expect(readFileSync(filePath, "utf8").trim()).toBe("");
  }));

test("truncateClaudeTranscript surfaces an unreadable transcript", async () =>
  withTempDir("sepia-claude-rewind-", async (root) => {
    const projectsDir = join(root, "projects");
    // A `.jsonl` that is a directory scans as a transcript but can't be read.
    mkdirSync(join(projectsDir, "-work-proj", "sess-1.jsonl"), { recursive: true });
    const result = await runEither(
      ClaudeCodeRepository.truncateClaudeTranscript({ projectsDir }, "sess-1", [], []),
    );
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.message).toContain("Failed to truncate");
  }));
