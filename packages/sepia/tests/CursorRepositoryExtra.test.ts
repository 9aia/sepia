/**
 * CursorRepository degradation paths the happy-path suite doesn't reach:
 * stat failures inside `scanEntries`, the injected `openStoreDb` failing or
 * throwing on `all`, and the transcript-shadowing rules in `getById`.
 */
import { Effect, Option } from "effect";
import { expect, test } from "vite-plus/test";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as CursorRepository from "../src/CursorRepository.js";
import type { CursorStoreDb } from "../src/CursorRepository.js";
import { MessageNode, Session, StorageError } from "../src/Domain.js";

const withTempDir = async <T>(prefix: string, fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const transcript = JSON.stringify({
  role: "user",
  message: { content: [{ type: "text", text: "hi there" }] },
});

const writeTranscript = (cursorDir: string, slug: string, chatId: string): void => {
  const dir = join(cursorDir, "projects", slug, "agent-transcripts", chatId);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `${chatId}.jsonl`), transcript + "\n");
};

/** `openStoreDb` backed by canned query results (or a throwing `all`). */
const fakeDb = (
  rows: Record<string, ReadonlyArray<Record<string, unknown>>>,
  opts: { allThrows?: boolean } = {},
): CursorRepository.OpenStoreDb => {
  return (_path) =>
    Effect.sync((): CursorStoreDb => ({
      all: (sql) => {
        if (opts.allThrows === true) throw new Error("no such table");
        const key = Object.keys(rows).find((k) => sql.includes(k));
        return key === undefined ? [] : rows[key]!;
      },
      close: () => {},
    }));
};

const session = (over: Partial<Parameters<typeof Session.make>[0]> = {}): Session =>
  Session.make({
    id: "s1",
    title: "t",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 2,
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

test("scanEntries skips non-dirs, unstatable entries and non-transcript files", async () =>
  withTempDir("sepia-cursor-repo-", async (cursorDir) => {
    const chats = join(cursorDir, "chats");
    mkdirSync(chats, { recursive: true });
    // a workspace-hash entry that is a file, and one that is a dangling link
    writeFileSync(join(chats, "a-file"), "x");
    symlinkSync(join(chats, "gone"), join(chats, "loop"));
    // a real workspace with a file instead of a chat dir, plus a bare dir
    const ws = join(chats, "aaaa");
    mkdirSync(ws);
    writeFileSync(join(ws, "not-a-chat"), "x");
    const chatDir = join(ws, "chat-1");
    mkdirSync(chatDir);
    writeFileSync(
      join(chatDir, "meta.json"),
      JSON.stringify({
        schemaVersion: 1,
        createdAtMs: 1_700_000_000_000,
        title: "meta only",
        hasConversation: false,
      }),
    );

    // a transcript chat dir holding only subagents and a non-jsonl file
    const projects = join(cursorDir, "projects");
    const tDir = join(projects, "home-x", "agent-transcripts", "chat-2");
    mkdirSync(join(tDir, "subagents"), { recursive: true });
    writeFileSync(join(tDir, "subagents", "notes.txt"), "x");
    writeFileSync(join(tDir, "subagents", "agent-1.jsonl"), transcript + "\n");
    // a transcript entry that is a file, not a dir
    writeFileSync(join(projects, "home-x", "agent-transcripts", "loose"), "x");

    const repo = CursorRepository.makeCursorSessionRepository({ cursorDir });
    const sessions = await Effect.runPromise(repo.list());
    const ids = sessions.map((s) => s.id);
    expect(ids).toContain("chat-1");
    expect(ids).toContain("agent-1");
    // the parent chat-2 has no main transcript — only the subagent lists
    expect(ids).not.toContain("chat-2");
    expect(Option.getOrUndefined(sessions.find((s) => s.id === "agent-1")!.parentSessionId)).toBe(
      "chat-2",
    );
  }));

test("list degrades an unreadable workspace dir and a symlink-loop transcript", async () =>
  withTempDir("sepia-cursor-repo-", async (cursorDir) => {
    const chats = join(cursorDir, "chats");
    const locked = join(chats, "locked");
    mkdirSync(locked, { recursive: true });
    mkdirSync(join(locked, "chat-1"));
    writeFileSync(join(locked, "chat-1", "meta.json"), "{}");
    chmodSync(locked, 0o000);

    // a transcript whose main file is a symlink loop — exists() fails
    const chatDir = join(cursorDir, "projects", "home-x", "agent-transcripts", "chat-3");
    mkdirSync(chatDir, { recursive: true });
    const loop = join(chatDir, "chat-3.jsonl");
    symlinkSync(loop, loop);

    const repo = CursorRepository.makeCursorSessionRepository({ cursorDir });
    try {
      const sessions = await Effect.runPromise(repo.list());
      expect(sessions.map((s) => s.id)).toEqual([]);
    } finally {
      chmodSync(locked, 0o755);
    }
  }));

test("a throwing store.db degrades the chat summary to meta-only", async () =>
  withTempDir("sepia-cursor-repo-", async (cursorDir) => {
    const chatDir = join(cursorDir, "chats", "aaaa", "chat-1");
    mkdirSync(chatDir, { recursive: true });
    writeFileSync(join(chatDir, "store.db"), "");
    writeFileSync(
      join(chatDir, "meta.json"),
      JSON.stringify({
        schemaVersion: 1,
        createdAtMs: 1_700_000_000_000,
        title: "meta title",
        hasConversation: true,
        cwd: "/home/x",
      }),
    );

    // `all` throws on every query — allSafe swallows it and the meta.json
    // sidecar still carries the summary.
    const repo = CursorRepository.makeCursorSessionRepository({
      cursorDir,
      openStoreDb: fakeDb({}, { allThrows: true }),
    });
    const sessions = await Effect.runPromise(repo.list());
    expect(sessions[0]!.id).toBe("chat-1");
    expect(sessions[0]!.title).toBe("meta title");

    // the db itself failing to open degrades the same way
    const failing = CursorRepository.makeCursorSessionRepository({
      cursorDir,
      openStoreDb: () => Effect.fail(new StorageError({ message: "corrupt" })),
    });
    const again = await Effect.runPromise(failing.list());
    expect(again[0]!.title).toBe("meta title");
  }));

test("getById prefers the chat over a same-id transcript and finds subagents", async () =>
  withTempDir("sepia-cursor-repo-", async (cursorDir) => {
    const chatDir = join(cursorDir, "chats", "aaaa", "dup");
    mkdirSync(chatDir, { recursive: true });
    writeFileSync(
      join(chatDir, "meta.json"),
      JSON.stringify({
        schemaVersion: 1,
        createdAtMs: 1,
        title: "chat wins",
        hasConversation: true,
      }),
    );
    writeTranscript(cursorDir, "home-x", "dup");
    writeTranscript(cursorDir, "home-x", "solo");

    const repo = CursorRepository.makeCursorSessionRepository({ cursorDir });
    // the chat has no store.db — the meta-only summary carries zero nodes,
    // so the transcript's fuller projection wins `getById`
    const found = await Effect.runPromise(repo.getById("dup"));
    expect(Option.getOrThrow(found).title).toBe("hi there");
    expect(Option.getOrThrow(found).metadata).toMatchObject({ store: "transcript" });

    const solo = await Effect.runPromise(repo.getById("solo"));
    expect(Option.getOrThrow(solo).metadata).toMatchObject({ store: "transcript" });
    expect(Option.isNone(await Effect.runPromise(repo.getById("ghost")))).toBe(true);
  }));

test("save refuses unsafe ids and survives a broken parent probe", async () =>
  withTempDir("sepia-cursor-repo-", async (cursorDir) => {
    const repo = CursorRepository.makeCursorSessionRepository({
      cursorDir,
      openStoreDb: fakeDb({}),
      openWritableStoreDb: fakeDb({}),
    });

    await expect(Effect.runPromise(repo.save(session({ id: "a/b" })))).rejects.toThrow(
      "not a safe file name",
    );
    await expect(
      Effect.runPromise(repo.save(session({ parentSessionId: Option.some("../evil") }))),
    ).rejects.toThrow("not a safe file name");

    // a subagent whose parent-dir probe hits a symlink loop degrades to the
    // session's own project slug
    const projDir = join(cursorDir, "projects", "loop-proj", "agent-transcripts");
    mkdirSync(projDir, { recursive: true });
    const loop = join(projDir, "parent-1");
    symlinkSync(loop, loop);
    await Effect.runPromise(
      repo.save(session({ id: "agent-9", parentSessionId: Option.some("parent-1") })),
    );
    const expected = join(
      cursorDir,
      "projects",
      "w",
      "agent-transcripts",
      "parent-1",
      "subagents",
      "agent-9.jsonl",
    );
    expect(existsSync(expected)).toBe(true);
  }));

test("save registers a store write failure as a StorageError", async () =>
  withTempDir("sepia-cursor-repo-", async (cursorDir) => {
    const repo = CursorRepository.makeCursorSessionRepository({
      cursorDir,
      openStoreDb: fakeDb({}),
      openWritableStoreDb: () =>
        Effect.succeed({
          all: () => [],
          run: () => {
            throw new Error("readonly db");
          },
          close: () => {},
        }),
    });
    await expect(Effect.runPromise(repo.save(session()))).rejects.toThrow(
      "Failed to write cursor store",
    );
  }));
