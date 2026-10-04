import { Effect, Option } from "effect";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { expect, test } from "vite-plus/test";
import * as Cursor from "../src/Cursor.js";
import * as CursorRepository from "../src/CursorRepository.js";
import { StorageError } from "../src/Domain.js";

/* ------------------------------------------------------------- */
/* fixture helpers                                                */
/* ------------------------------------------------------------- */

const enc = new TextEncoder();
const bytes = (text: string): Uint8Array => enc.encode(text);
const json = (value: unknown): Uint8Array => bytes(JSON.stringify(value));
const hexJson = (value: unknown): string => Buffer.from(JSON.stringify(value)).toString("hex");

/** Deterministic 32-byte blob ids — the store only needs 64 hex chars. */
const blobId = (n: number): string => n.toString(16).padStart(64, "0");
const idBytes = (id: string): Uint8Array =>
  new Uint8Array(32).map((_, i) => Number.parseInt(id.slice(i * 2, i * 2 + 2), 16));

const varint = (n: number): Array<number> => {
  const out: Array<number> = [];
  do {
    let b = n & 0x7f;
    n = Math.floor(n / 128);
    if (n > 0) b |= 0x80;
    out.push(b);
  } while (n > 0);
  return out;
};

const field = (num: number, payload: Uint8Array): Uint8Array => {
  const head = varint(num * 8 + 2);
  const len = varint(payload.length);
  const out = new Uint8Array(head.length + len.length + payload.length);
  out.set(head, 0);
  out.set(len, head.length);
  out.set(payload, head.length + len.length);
  return out;
};

const varintField = (num: number, value: number): Uint8Array => {
  const head = varint(num * 8);
  const v = varint(value);
  const out = new Uint8Array(head.length + v.length);
  out.set(head, 0);
  out.set(v, head.length);
  return out;
};

const concat = (...parts: ReadonlyArray<Uint8Array>): Uint8Array => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};

/** A checkpoint blob: f1 refs, f9 workspace uri, f10 flag, f22 client. */
const checkpoint = (
  messageIds: ReadonlyArray<string>,
  workspace?: string,
  client = "cli",
): Uint8Array =>
  concat(
    ...messageIds.map((id) => field(1, idBytes(id))),
    ...(workspace === undefined ? [] : [field(9, bytes(workspace))]),
    varintField(10, 1),
    field(22, bytes(client)),
  );

const META = {
  agentId: "chat-1",
  latestRootBlobId: blobId(0),
  name: "Store Chat",
  mode: "default",
  isRunEverything: false,
  createdAt: 1_700_000_000_000,
  lastUsedModel: "composer-1",
};

const storeMessages = () => ({
  system: json({ role: "system", content: "You are an AI coding assistant." }),
  userInfo: json({ role: "user", content: "<user_info>\nOS Version: linux\n</user_info>" }),
  prompt: json({
    role: "user",
    content: [{ type: "text", text: "<user_query>\ntake a screenshot\n</user_query>" }],
    providerOptions: { cursor: { requestId: "req-1" } },
  }),
  assistant: json({
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
  result: json({
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
          output: {
            success: { command: "grim shot.png", executionTime: 5285 },
            isError: false,
          },
        },
      },
    },
  }),
  opaque: bytes("\x0a\x05not-a-message"),
});

const storeBlobs = (includeRoot = true): Map<string, Uint8Array> => {
  const msgs = storeMessages();
  const blobs = new Map<string, Uint8Array>();
  blobs.set(blobId(1), msgs.system);
  blobs.set(blobId(2), msgs.userInfo);
  blobs.set(blobId(3), msgs.prompt);
  blobs.set(blobId(4), msgs.assistant);
  blobs.set(blobId(5), msgs.result);
  blobs.set(blobId(6), msgs.opaque);
  if (includeRoot) {
    blobs.set(
      blobId(0),
      checkpoint(
        [blobId(1), blobId(2), blobId(3), blobId(4), blobId(5), blobId(6), blobId(9)],
        "file:///home/luis/Desktop",
      ),
    );
  }
  return blobs;
};

/* ------------------------------------------------------------- */
/* store meta + checkpoint decoding                               */
/* ------------------------------------------------------------- */

test("parseStoreMeta decodes hex-encoded and plain JSON", () => {
  const meta = Cursor.parseStoreMeta(hexJson(META));
  expect(meta).toEqual({
    agentId: "chat-1",
    latestRootBlobId: blobId(0),
    name: "Store Chat",
    mode: "default",
    isRunEverything: false,
    createdAt: 1_700_000_000_000,
    lastUsedModel: "composer-1",
  });
  expect(Cursor.parseStoreMeta(JSON.stringify({ name: "plain" }))).toEqual({
    agentId: undefined,
    latestRootBlobId: undefined,
    name: "plain",
    mode: undefined,
    isRunEverything: false,
    createdAt: undefined,
    lastUsedModel: undefined,
  });
  expect(Cursor.parseStoreMeta("not json at all")).toBeUndefined();
  expect(Cursor.parseStoreMeta("zzzz")).toBeUndefined();
  expect(Cursor.parseStoreMeta(42)).toBeUndefined();
  expect(Cursor.parseStoreMeta("")).toBeUndefined();
});

test("decodeCheckpoint reads ordered refs, workspace and client", () => {
  const blob = checkpoint([blobId(1), blobId(2)], "file:///home/luis/proj");
  expect(Cursor.decodeCheckpoint(blob)).toEqual({
    messageIds: [blobId(1), blobId(2)],
    workspace: "file:///home/luis/proj",
    client: "cli",
  });
  // empty blob and non-protobuf junk are not checkpoints
  expect(Cursor.decodeCheckpoint(new Uint8Array(0))).toBeUndefined();
  expect(Cursor.decodeCheckpoint(bytes("\xff\xff\xff\xff"))).toBeUndefined();
});

test("workspaceFromUri decodes file:// uris only", () => {
  expect(Cursor.workspaceFromUri("file:///home/luis/My%20Proj")).toBe("/home/luis/My Proj");
  expect(Cursor.workspaceFromUri("vscode://x/y")).toBeUndefined();
  expect(Cursor.workspaceFromUri(undefined)).toBeUndefined();
  expect(Cursor.workspaceFromUri("file://%zz")).toBeUndefined();
});

test("extractUserQuery unwraps the tag or strips markup", () => {
  expect(Cursor.extractUserQuery("<user_query>\nhello there\n</user_query>")).toBe("hello there");
  // tag markup is stripped; unattributed inner text is kept
  expect(Cursor.extractUserQuery("<attached_files>x</attached_files> plain text")).toBe(
    "x plain text",
  );
  expect(Cursor.extractUserQuery("   ")).toBeUndefined();
});

/* ------------------------------------------------------------- */
/* store.db → IR                                                  */
/* ------------------------------------------------------------- */

test("sessionFromStore decodes the checkpoint's ordered messages", () => {
  const session = Cursor.sessionFromStore({
    id: "chat-1",
    workspaceHash: "deadbeef",
    meta: META,
    blobs: storeBlobs(),
    promptHistory: ["take a screenshot"],
  });

  expect(session.id).toBe("chat-1");
  expect(session.title).toBe("Store Chat");
  expect(session.workingDirectory).toBe("/home/luis/Desktop");
  expect(session.backendType).toBe("cursor");
  expect(session.model).toBe("composer-1");
  expect(session.createdAt).toBe(1_700_000_000);
  expect(session.lastActivityAt).toBe(1_700_000_000);
  expect(session.promptHistory.map((p) => p.content)).toEqual(["take a screenshot"]);

  const meta = session.metadata as Record<string, unknown>;
  expect(meta.source).toBe("cursor");
  expect(meta.store).toBe("chats");
  expect(meta.workspaceHash).toBe("deadbeef");
  expect(meta.client).toBe("cli");
  // the opaque projection blob and the dangling blobId(9) ref
  expect(meta.opaqueBlobs).toBe(2);

  const [system, userInfo, prompt, assistant, tool] = session.nodes;
  expect(session.nodes).toHaveLength(5);
  expect(system.role).toBe("system");
  expect(userInfo.role).toBe("user");
  expect((userInfo.metadata as Record<string, unknown>).context).toBe("user_info");

  expect(prompt.role).toBe("user");
  expect(prompt.content).toContain("take a screenshot");
  expect(Option.getOrUndefined(prompt.requestId)).toBe("req-1");

  expect(assistant.role).toBe("assistant");
  expect(assistant.content).toContain("Taking a screenshot.");
  expect(Option.getOrUndefined(assistant.thinking)).toBe(Cursor.REDACTED_THINKING);
  // the opaque redacted-reasoning blob rides verbatim as the seal
  expect(Option.getOrUndefined(assistant.thinkingSignature)).toBe("opaque-payload");
  expect(assistant.toolCalls).toHaveLength(1);
  expect(assistant.toolCalls[0]).toMatchObject({
    id: "tool_1",
    name: "Shell",
    arguments: { command: "grim shot.png" },
  });
  // the tool result's success folds back onto the call
  expect(Option.getOrUndefined(assistant.toolCalls[0].status)).toBe("success");
  expect(Option.getOrUndefined(assistant.toolCalls[0].durationMs)).toBe(5285);

  expect(tool.role).toBe("tool");
  expect(Option.getOrUndefined(tool.toolCallId)).toBe("tool_1");
  expect(Option.getOrUndefined(tool.toolName)).toBe("Shell");
  expect(Option.getOrUndefined(tool.toolResult)?.status).toBe("success");
  expect(Option.getOrUndefined(tool.toolResult)?.durationMs).toBe(5285);
  expect(tool.content).toContain("Exit code: 0");
  expect((tool.metadata as Record<string, unknown>).toolArguments).toEqual({
    command: "grim shot.png",
  });

  // linear chain
  for (let i = 1; i < session.nodes.length; i++) {
    expect(Option.getOrUndefined(session.nodes[i].parentNodeId)).toBe(i - 1);
  }
  expect(Option.isNone(session.nodes[0].parentNodeId)).toBe(true);
});

test("sessionFromStore marks error results and metaJson overrides", () => {
  const errResult = json({
    role: "tool",
    content: [
      { type: "tool-result", toolCallId: "tool_1", toolName: "Shell", result: { failed: true } },
    ],
    providerOptions: { cursor: { highLevelToolCallResult: { output: { isError: true } } } },
  });
  const blobs = new Map(storeBlobs());
  blobs.set(blobId(5), errResult);
  const session = Cursor.sessionFromStore({
    id: "chat-1",
    meta: META,
    metaJson: { title: "Meta Title", cwd: "/override/cwd", updatedAtMs: 1_700_100_000_000 },
    blobs,
  });
  expect(session.title).toBe("Meta Title");
  expect(session.workingDirectory).toBe("/override/cwd");
  expect(session.lastActivityAt).toBe(1_700_100_000);
  const tool = session.nodes[4];
  expect(Option.getOrUndefined(tool.toolResult)?.status).toBe("error");
  expect(Option.getOrUndefined(tool.toolResult)?.durationMs).toBeUndefined();
  expect(Option.getOrUndefined(session.nodes[3].toolCalls[0].status)).toBe("error");
  expect(tool.content).toContain("failed");
});

test("sessionFromStore falls back without a checkpoint root", () => {
  const blobs = new Map<string, Uint8Array>();
  blobs.set(blobId(1), json({ role: "user", content: "orphan" }));
  const session = Cursor.sessionFromStore({
    id: "chat-x",
    meta: { name: "No Root" },
    blobs,
  });
  expect(session.title).toBe("No Root");
  expect(session.nodes).toEqual([]);
  expect(session.workingDirectory).toBe("/");
  expect(session.model).toBe("unknown");
});

test("sessionFromStore titles from the first user query when unnamed", () => {
  const session = Cursor.sessionFromStore({ id: "chat-1", blobs: storeBlobs() });
  expect(session.title).toBe("take a screenshot");
  expect(session.metadata as Record<string, unknown>).toMatchObject({
    agentId: null,
    latestRootBlobId: null,
  });
});

test("summarizeStore prefers metaJson fields and tolerates missing data", () => {
  const s = Cursor.summarizeStore({
    id: "c1",
    meta: META,
    metaJson: { title: "Json Title", cwd: "/json/cwd", createdAtMs: 1_700_000_500_000 },
    workspace: "/ws/cwd",
  });
  expect(s.title).toBe("Json Title");
  expect(s.workingDirectory).toBe("/json/cwd");
  expect(s.createdAt).toBe(1_700_000_500);
  expect(s.nodes).toEqual([]);

  const bare = Cursor.summarizeStore({ id: "c2", mtimeMs: 1_700_000_000_000 });
  expect(bare.title).toBe("c2");
  expect(bare.workingDirectory).toBe("/");
  expect(bare.model).toBe("unknown");
});

/* ------------------------------------------------------------- */
/* agent-transcripts projection                                   */
/* ------------------------------------------------------------- */

const transcript = [
  JSON.stringify({
    role: "user",
    message: { content: [{ type: "text", text: "<user_query>\nexplore the repo\n</user_query>" }] },
  }),
  JSON.stringify({
    role: "assistant",
    message: {
      content: [
        { type: "text", text: "I'll look around.\n\n[REDACTED]" },
        { type: "tool_use", name: "Glob", input: { glob_pattern: "src/**" } },
      ],
    },
  }),
  '{"role":"assistant" BAD',
  JSON.stringify({ type: "turn_ended", status: "error", error: { message: "rate limited" } }),
].join("\n");

test("fromTranscriptJsonl maps the lossy projection honestly", () => {
  const session = Cursor.fromTranscriptJsonl(transcript, {
    id: "chat-9",
    projectSlug: "home-luis-Desktop-cheloni-v4",
    mtimeMs: 1_700_000_000_000,
  });
  expect(session.id).toBe("chat-9");
  expect(session.title).toBe("explore the repo");
  expect(session.workingDirectory).toBe("/home/luis/Desktop/cheloni/v4");
  expect(session.backendType).toBe("cursor");
  expect(session.createdAt).toBe(1_700_000_000);
  const meta = session.metadata as Record<string, unknown>;
  expect(meta.store).toBe("transcript");
  expect(meta.lossy).toBe(true);
  expect(meta.turnErrors).toEqual([{ message: "rate limited" }]);

  const [user, assistant] = session.nodes;
  expect(user.role).toBe("user");
  expect(assistant.content).toBe("I'll look around.");
  expect(Option.getOrUndefined(assistant.thinking)).toBe(Cursor.REDACTED_THINKING);
  // the lossy projection keeps no blob — marker only, no signature
  expect(Option.isNone(assistant.thinkingSignature)).toBe(true);
  expect(assistant.toolCalls[0]).toMatchObject({
    name: "Glob",
    arguments: { glob_pattern: "src/**" },
  });
  expect(assistant.toolCalls[0].id).toMatch(/^cursor-tool-/);
  expect(session.promptHistory.map((p) => p.content)).toEqual(["explore the repo"]);
});

test("transcript subagent source records the parent chat", () => {
  const session = Cursor.fromTranscriptJsonl(transcript, {
    id: "sub-1",
    projectSlug: "home-luis-proj",
    parentSessionId: "chat-9",
  });
  expect(Option.getOrUndefined(session.parentSessionId)).toBe("chat-9");

  const summary = Cursor.summarizeTranscriptJsonl(transcript, {
    id: "sub-1",
    projectSlug: "home-luis-proj",
    parentSessionId: "chat-9",
  });
  expect(summary.title).toBe("explore the repo");
  expect(summary.nodes).toEqual([]);
});

test("empty and malformed transcripts yield a bare session", () => {
  const session = Cursor.fromTranscriptJsonl("\nnot json\n", {
    id: "t",
    projectSlug: "home-x",
  });
  expect(session.nodes).toEqual([]);
  expect(session.title).toBe("t");
  expect(session.promptHistory).toEqual([]);
});

/* ------------------------------------------------------------- */
/* repository                                                     */
/* ------------------------------------------------------------- */

interface FakeStore {
  readonly meta: string | undefined;
  readonly blobs: Map<string, Uint8Array>;
}

const fakeStoreDb = (stores: Map<string, FakeStore>): CursorRepository.OpenStoreDb => {
  return (path) => {
    const store = stores.get(path);
    if (store === undefined) {
      return Effect.fail(new StorageError({ message: `no store at ${path}` }));
    }
    return Effect.succeed({
      all: (sql, params = []) => {
        if (sql.includes("from meta")) {
          return store.meta === undefined ? [] : [{ value: store.meta }];
        }
        if (sql.includes("where id")) {
          const data = store.blobs.get(params[0] as string);
          return data === undefined ? [] : [{ data }];
        }
        return [...store.blobs.entries()].map(([id, data]) => ({ id, data }));
      },
      close: () => {},
    });
  };
};

const makeTree = (files: Record<string, string>): string => {
  const root = mkdtempSync(join(tmpdir(), "sepia-cursor-"));
  for (const [rel, content] of Object.entries(files)) {
    const filePath = join(root, rel);
    mkdirSync(dirname(filePath), { recursive: true });
    writeFileSync(filePath, content);
  }
  return root;
};

const transcriptLine = (role: string, text: string) =>
  JSON.stringify({ role, message: { content: [{ type: "text", text }] } });

const layout = (root: string): Map<string, FakeStore> =>
  new Map([
    [join(root, "chats/wsHASH/chat-1/store.db"), { meta: hexJson(META), blobs: storeBlobs() }],
    [
      join(root, "chats/wsHASH/chat-pruned/store.db"),
      // sha256("") is the only blob a fully pruned store keeps
      {
        meta: hexJson({ ...META, latestRootBlobId: blobId(42), name: "Pruned" }),
        blobs: new Map([[blobId(42), new Uint8Array(0)]]),
      },
    ],
  ]);

const fixture = () => {
  const root = makeTree({
    "chats/wsHASH/chat-1/store.db": "",
    "chats/wsHASH/chat-1/meta.json": JSON.stringify({
      schemaVersion: 1,
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_100_000_000,
      title: "Release Clock Time",
      hasConversation: true,
    }),
    "chats/wsHASH/chat-1/prompt_history.json": JSON.stringify(["release it"]),
    "chats/wsHASH/meta-only/meta.json": JSON.stringify({
      createdAtMs: 1_699_000_000_000,
      title: "Empty Chat",
      hasConversation: false,
      cwd: "/home/luis",
    }),
    "chats/wsHASH/stray-file.txt": "not a dir",
    // transcript for the same chat — the store.db wins on the shared id
    "projects/home-luis-Desktop/agent-transcripts/chat-1/chat-1.jsonl": transcriptLine(
      "user",
      "dup",
    ),
    // pruned store: an empty checkpoint only — the transcript wins on read
    "chats/wsHASH/chat-pruned/store.db": "",
    "projects/home-luis-Desktop/agent-transcripts/chat-pruned/chat-pruned.jsonl": transcript,
    "projects/home-luis-Desktop/agent-transcripts/chat-2/chat-2.jsonl": transcript,
    "projects/home-luis-Desktop/agent-transcripts/chat-2/subagents/sub-1.jsonl": transcriptLine(
      "user",
      "<user_query>sub task</user_query>",
    ),
    "projects/home-luis-Desktop/agent-transcripts/chat-2/notes.txt": "ignored",
    "projects/not-transcripts/readme.md": "ignored",
  });
  const stores = layout(root);
  return {
    repo: CursorRepository.makeCursorSessionRepository({
      cursorDir: root,
      openStoreDb: fakeStoreDb(stores),
    }),
    root,
  };
};

test("repository lists chat stores, meta-only chats and transcripts", async () => {
  const { repo } = fixture();
  const sessions = await Effect.runPromise(repo.list());
  const byId = new Map(sessions.map((s) => [s.id, s]));

  // deduped: one chat-1, store-backed (meta.json title)
  expect(sessions.filter((s) => s.id === "chat-1")).toHaveLength(1);
  const chat1 = byId.get("chat-1");
  expect(chat1?.title).toBe("Release Clock Time");
  expect(chat1?.backendType).toBe("cursor");
  expect(chat1?.model).toBe("composer-1");
  // workspace comes from the root checkpoint's file:// uri
  expect(chat1?.workingDirectory).toBe("/home/luis/Desktop");

  const metaOnly = byId.get("meta-only");
  expect(metaOnly?.title).toBe("Empty Chat");
  expect(metaOnly?.workingDirectory).toBe("/home/luis");

  const chat2 = byId.get("chat-2");
  expect(chat2?.title).toBe("explore the repo");
  expect((chat2?.metadata as Record<string, unknown> | undefined)?.store).toBe("transcript");

  const sub = byId.get("sub-1");
  expect(Option.getOrUndefined(sub?.parentSessionId ?? Option.none())).toBe("chat-2");

  // sorted by activity desc
  for (let i = 1; i < sessions.length; i++) {
    expect(sessions[i - 1].lastActivityAt).toBeGreaterThanOrEqual(sessions[i].lastActivityAt);
  }
});

test("repository getById decodes store blobs fully", async () => {
  const { repo } = fixture();
  const found = await Effect.runPromise(repo.getById("chat-1"));
  expect(Option.isSome(found)).toBe(true);
  const session = Option.getOrThrow(found);
  expect(session.nodes).toHaveLength(5);
  expect(session.promptHistory.map((p) => p.content)).toEqual(["release it"]);

  const metaOnly = await Effect.runPromise(repo.getById("meta-only"));
  expect(Option.getOrThrow(metaOnly).title).toBe("Empty Chat");

  const chat2 = await Effect.runPromise(repo.getById("chat-2"));
  expect(Option.getOrThrow(chat2).nodes.length).toBeGreaterThan(0);

  const sub = await Effect.runPromise(repo.getById("sub-1"));
  expect(Option.getOrUndefined(Option.getOrThrow(sub).parentSessionId)).toBe("chat-2");

  expect(Option.isNone(await Effect.runPromise(repo.getById("ghost")))).toBe(true);
});

test("repository getById prefers the transcript when the store is pruned", async () => {
  const { repo } = fixture();
  const found = await Effect.runPromise(repo.getById("chat-pruned"));
  const session = Option.getOrThrow(found);
  expect(session.nodes.length).toBeGreaterThan(0);
  expect(session.nodes[0].role).toBe("user");
  expect((session.metadata as Record<string, unknown>).store).toBe("transcript");
});

test("repository hasSession and read-only writes", async () => {
  const { repo } = fixture();
  await expect(Effect.runPromise(repo.hasSession("chat-1"))).resolves.toBe(true);
  await expect(Effect.runPromise(repo.hasSession("sub-1"))).resolves.toBe(true);
  await expect(Effect.runPromise(repo.hasSession("nope"))).resolves.toBe(false);
  await expect(Effect.runPromise(repo.save({} as never))).rejects.toThrow(
    "Cursor repository is read-only",
  );
  await expect(Effect.runPromise(repo.delete("x"))).rejects.toThrow(
    "Cursor repository is read-only",
  );
});

test("repository treats a missing cursor dir as empty", async () => {
  const repo = CursorRepository.makeCursorSessionRepository({
    cursorDir: join(tmpdir(), "sepia-cursor-does-not-exist"),
    openStoreDb: fakeStoreDb(new Map()),
  });
  expect(await Effect.runPromise(repo.list())).toEqual([]);
  expect(Option.isNone(await Effect.runPromise(repo.getById("x")))).toBe(true);
});

test("repository degrades a broken store.db to meta-only listing", async () => {
  const root = makeTree({
    "chats/wsHASH/chat-1/store.db": "",
    "chats/wsHASH/chat-1/meta.json": JSON.stringify({
      createdAtMs: 1_700_000_000_000,
      title: "Still Listed",
      cwd: "/home/luis",
    }),
  });
  const repo = CursorRepository.makeCursorSessionRepository({
    cursorDir: root,
    // every store.db open fails — listing must not go down with it
    openStoreDb: () => Effect.fail(new StorageError({ message: "corrupt" })),
  });
  const sessions = await Effect.runPromise(repo.list());
  expect(sessions).toHaveLength(1);
  expect(sessions[0].title).toBe("Still Listed");
});
