import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Option } from "effect";
import { describe, expect, test } from "vite-plus/test";
import { MessageNode, Rewind, Session } from "sepia-core";
import {
  makeClaudeCodeSessionRepository,
  truncateClaudeTranscript,
} from "../src/ClaudeCodeRepository.js";

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-claude-repo-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const claudeLine = (entry: Record<string, unknown>): string => JSON.stringify(entry);

const transcript = (id: string, cwd: string) =>
  [
    claudeLine({ type: "summary", summary: "T", leafUuid: "u1" }),
    claudeLine({
      type: "user",
      uuid: "u1",
      parentUuid: null,
      sessionId: id,
      cwd,
      timestamp: "2026-01-01T00:00:01.000Z",
      message: { role: "user", content: "prompt one" },
    }),
    claudeLine({
      type: "assistant",
      uuid: "u2",
      parentUuid: "u1",
      sessionId: id,
      timestamp: "2026-01-01T00:00:02.000Z",
      message: { role: "assistant", content: [{ type: "text", text: "answer one" }] },
    }),
  ].join("\n") + "\n";

const writeTranscript = (projectsDir: string, slug: string, id: string, cwd: string): string => {
  const dir = join(projectsDir, slug);
  mkdirSync(dir, { recursive: true });
  const filePath = join(dir, `${id}.jsonl`);
  writeFileSync(filePath, transcript(id, cwd));
  return filePath;
};

const sessionFixture = (id: string, cwd: string, parent?: string): Session =>
  Session.make({
    id,
    title: "t",
    workingDirectory: cwd,
    backendType: "claude",
    agentMode: "accept-edits",
    model: "m",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_100,
    mainChainId: 0,
    shellLastSeenIndex: 0,
    cogsJson: "[]",
    workspaceDirs: "[]",
    hidden: 0,
    metadata: null,
    ...(parent === undefined ? {} : { parentSessionId: Option.some(parent) }),
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "prompt",
        createdAt: 1_700_000_000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "assistant",
        content: "answer",
        createdAt: 1_700_000_050,
        metadata: null,
      }),
    ],
    promptHistory: [],
  });

describe("ClaudeCodeRepository", () => {
  test("list summarizes every transcript incl. subagent dirs", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      writeTranscript(projectsDir, "-work-a", "sess-a", "/work/a");
      writeTranscript(projectsDir, "-work-b", "sess-b", "/work/b");
      // subagent transcript under <uuid>/subagents/
      mkdirSync(join(projectsDir, "-work-a", "sess-a", "subagents"), { recursive: true });
      writeFileSync(
        join(projectsDir, "-work-a", "sess-a", "subagents", "agent-9.jsonl"),
        transcript("agent-9", "/work/a"),
      );

      const repo = makeClaudeCodeSessionRepository({ projectsDir });
      const sessions = await Effect.runPromise(repo.list());
      const ids = sessions.map((s) => s.id).sort();
      expect(ids).toEqual(["agent-9", "sess-a", "sess-b"]);
      const a = sessions.find((s) => s.id === "sess-a");
      expect(a?.title).toBe("T");
      expect(a?.workingDirectory).toBe("/work/a");
      const sub = sessions.find((s) => s.id === "agent-9");
      expect(Option.getOrUndefined(sub?.parentSessionId ?? Option.none())).toBe("sess-a");
    }));

  test("getById parses the transcript; hasSession scans ids", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      writeTranscript(projectsDir, "-work-a", "sess-a", "/work/a");
      const repo = makeClaudeCodeSessionRepository({ projectsDir });

      const found = await Effect.runPromise(repo.getById("sess-a"));
      expect(Option.isSome(found)).toBe(true);
      if (Option.isSome(found)) {
        expect(found.value.nodes.length).toBe(2);
        expect(found.value.nodes[1]?.content).toBe("answer one");
      }
      expect(Option.isNone(await Effect.runPromise(repo.getById("missing")))).toBe(true);
      expect(await Effect.runPromise(repo.hasSession("sess-a"))).toBe(true);
      expect(await Effect.runPromise(repo.hasSession("nope"))).toBe(false);
    }));

  test("an empty or missing projects root lists nothing", async () =>
    withTempDir(async (root) => {
      const repo = makeClaudeCodeSessionRepository({ projectsDir: join(root, "absent") });
      expect(await Effect.runPromise(repo.list())).toEqual([]);
      expect(await Effect.runPromise(repo.hasSession("x"))).toBe(false);
    }));

  test("save writes the canonical layout and delete removes it", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      const repo = makeClaudeCodeSessionRepository({ projectsDir });

      await Effect.runPromise(repo.save(sessionFixture("new-1", "/work/proj")));
      const filePath = join(projectsDir, "-work-proj", "new-1.jsonl");
      expect(readFileSync(filePath, "utf8").length).toBeGreaterThan(0);
      const reread = await Effect.runPromise(repo.getById("new-1"));
      expect(Option.isSome(reread)).toBe(true);

      // subagent lands under the parent's subagents dir
      await Effect.runPromise(repo.save(sessionFixture("agent-7", "/work/proj", "new-1")));
      const subPath = join(projectsDir, "-work-proj", "new-1", "subagents", "agent-7.jsonl");
      expect(readFileSync(subPath, "utf8").length).toBeGreaterThan(0);

      await Effect.runPromise(repo.delete("new-1"));
      expect(Option.isNone(await Effect.runPromise(repo.getById("new-1")))).toBe(true);
      // deleting again is a no-op
      await Effect.runPromise(repo.delete("new-1"));
    }));

  test("list caches summaries until the transcript set changes", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      const filePath = writeTranscript(projectsDir, "-work-a", "sess-a", "/work/a");
      // a dangling-symlink transcript is skipped by the scan and the stat
      symlinkSync(
        join(projectsDir, "-work-a", "nonexistent.jsonl"),
        join(projectsDir, "-work-a", "ghost.jsonl"),
      );
      const repo = makeClaudeCodeSessionRepository({ projectsDir });

      const first = await Effect.runPromise(repo.list());
      expect(first.map((s) => s.id)).toEqual(["sess-a"]);
      // unchanged stamp → the cached array comes back verbatim
      expect(await Effect.runPromise(repo.list())).toBe(first);

      // editing a transcript bumps its mtime → re-read
      writeFileSync(
        filePath,
        transcript("sess-a", "/work/a").replace('"summary":"T"', '"summary":"Edited"'),
      );
      utimesSync(filePath, new Date(), new Date(Date.now() + 10_000));
      const third = await Effect.runPromise(repo.list());
      expect(third).not.toBe(first);
      expect(third.find((s) => s.id === "sess-a")?.title).toBe("Edited");

      // a new transcript busts via the file-set signature
      writeTranscript(projectsDir, "-work-b", "sess-b", "/work/b");
      expect((await Effect.runPromise(repo.list())).map((s) => s.id).sort()).toEqual([
        "sess-a",
        "sess-b",
      ]);
    }));

  test("save refuses unsafe ids", async () =>
    withTempDir(async (root) => {
      const repo = makeClaudeCodeSessionRepository({ projectsDir: join(root, "p") });
      const result = await Effect.runPromise(
        repo.save(sessionFixture("../escape", "/w")).pipe(Effect.either),
      );
      expect(result._tag).toBe("Left");
    }));

  test("delete refuses unsafe ids", async () =>
    withTempDir(async (root) => {
      const repo = makeClaudeCodeSessionRepository({ projectsDir: join(root, "p") });
      const result = await Effect.runPromise(repo.delete("../x").pipe(Effect.either));
      expect(result._tag).toBe("Left");
    }));
});

describe("truncateClaudeTranscript", () => {
  test("rewrites the file to end at the cut, bytes of kept lines intact", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      const filePath = writeTranscript(projectsDir, "-work-proj", "sess-1", "/work/proj");
      // append a second turn to truncate away
      const extra = [
        claudeLine({
          type: "user",
          uuid: "u3",
          parentUuid: "u2",
          sessionId: "sess-1",
          timestamp: "2026-01-01T00:00:03.000Z",
          message: { role: "user", content: "prompt two" },
        }),
        claudeLine({
          type: "assistant",
          uuid: "u4",
          parentUuid: "u3",
          sessionId: "sess-1",
          timestamp: "2026-01-01T00:00:04.000Z",
          message: { role: "assistant", content: [{ type: "text", text: "answer two" }] },
        }),
      ].join("\n");
      const original = readFileSync(filePath, "utf8");
      writeFileSync(filePath, original + extra + "\n");

      const repo = makeClaudeCodeSessionRepository({ projectsDir });
      const session = await Effect.runPromise(repo.getById("sess-1"));
      expect(Option.isSome(session)).toBe(true);
      if (Option.isNone(session)) return;
      const target = session.value.nodes.find((n) => n.content === "answer one");
      expect(target).toBeDefined();
      const planned = Rewind.planRewind(session.value, { nodeId: target!.nodeId });
      expect(planned.ok).toBe(true);
      if (!planned.ok) return;

      const result = await Effect.runPromise(
        truncateClaudeTranscript(
          { projectsDir },
          "sess-1",
          planned.plan.kept,
          planned.plan.removed,
        ),
      );
      expect(result.removedEntries).toBe(2);
      expect(readFileSync(filePath, "utf8")).toBe(original);
    }));

  test("fails for an unknown session", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      mkdirSync(projectsDir, { recursive: true });
      const result = await Effect.runPromise(
        truncateClaudeTranscript({ projectsDir }, "ghost", [], []).pipe(Effect.either),
      );
      expect(result._tag).toBe("Left");
    }));

  test("a kept node with no locatable entry fails instead of guessing", async () =>
    withTempDir(async (root) => {
      const projectsDir = join(root, "projects");
      writeTranscript(projectsDir, "-work-proj", "sess-1", "/work/proj");
      const orphan = MessageNode.make({
        nodeId: 99,
        role: "user",
        content: "no uuid metadata",
        createdAt: 1,
        metadata: null,
      });
      const result = await Effect.runPromise(
        truncateClaudeTranscript({ projectsDir }, "sess-1", [orphan], []).pipe(Effect.either),
      );
      expect(result._tag).toBe("Left");
    }));
});
