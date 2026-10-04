import { Effect, Option } from "effect";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { expect, test } from "vite-plus/test";
import * as Cursor from "../src/Cursor.js";
import * as CursorRepository from "../src/CursorRepository.js";
import { MessageNode, Session, StorageError, ToolCall } from "../src/Domain.js";

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

test("transcript file-edit tool calls carry locations and revertable diffs", () => {
  const session = Cursor.fromTranscriptJsonl(
    [
      { role: "user", message: { content: [{ type: "text", text: "change it" }] } },
      {
        role: "assistant",
        message: {
          content: [
            {
              type: "tool_use",
              id: "t1",
              name: "StrReplace",
              input: { path: "/w/a.ts", old_string: "o", new_string: "n" },
            },
            {
              type: "tool_use",
              id: "t2",
              name: "Write",
              input: { path: "/w/b.ts", contents: "made" },
            },
            { type: "tool_use", id: "t3", name: "Delete", input: { path: "/w/c.ts" } },
            {
              type: "tool_use",
              id: "t4",
              name: "Glob",
              input: { target_directory: "/w", glob_pattern: "**/*" },
            },
            {
              type: "tool_use",
              id: "t5",
              name: "ApplyPatch",
              input:
                "*** Begin Patch\n" +
                "*** Update File: /w/d.ts\n" +
                "@@\n" +
                " ctx\n" +
                "-old\n" +
                "+new\n" +
                "*** Add File: /w/e.ts\n" +
                "+whole\n" +
                "+file\n" +
                "*** Delete File: /w/f.ts\n" +
                "*** End Patch",
            },
          ],
        },
      },
    ]
      .map((l) => JSON.stringify(l))
      .join("\n"),
    { id: "c1", projectSlug: "w" },
  );
  const [strReplace, write, del, glob, patch] = session.nodes[1].toolCalls;
  expect(strReplace.locations).toEqual([{ path: "/w/a.ts" }]);
  expect(strReplace.diffs).toEqual([{ path: "/w/a.ts", oldText: "o", newText: "n" }]);
  expect(write.diffs).toEqual([{ path: "/w/b.ts", newText: "made" }]);
  // a delete records the path it removed — nothing to revert with
  expect(del.locations).toEqual([{ path: "/w/c.ts" }]);
  expect(del.diffs).toEqual([]);
  expect(glob.locations).toEqual([{ path: "/w" }]);
  expect(patch.diffs).toEqual([
    { path: "/w/d.ts", oldText: "ctx\nold", newText: "ctx\nnew" },
    { path: "/w/e.ts", newText: "whole\nfile" },
    // a delete section with no `-` payload keeps the change on record
    { path: "/w/f.ts" },
  ]);
});

test("toolFileRefs covers renames, deletes, sentinel lines and edge args", () => {
  const refs = Cursor.toolFileRefs(
    "ApplyPatch",
    "@@ stray before any section\n" +
      "*** Begin Patch\n" +
      "*** Update File: /w/old.ts\n" +
      "*** Move to: /w/new.ts\n" +
      "@@ class Foo\n" +
      "-x\n" +
      "+y\n" +
      "\\ No newline at end of file\n" +
      "*** Delete File: /w/gone.ts\n" +
      "-gone body\n" +
      "*** End Patch",
  );
  expect(refs.diffs).toEqual([
    // hunks land on the rename target — the rename itself is no diff
    { path: "/w/new.ts", oldText: "x", newText: "y" },
    // a delete's `-` payload is the only content that can resurrect it
    { path: "/w/gone.ts", oldText: "gone body" },
  ]);
  expect(refs.locations).toEqual([
    { path: "/w/old.ts" },
    { path: "/w/new.ts" },
    { path: "/w/gone.ts" },
  ]);

  // args that carry no patch text
  expect(Cursor.toolFileRefs("ApplyPatch", {})).toEqual({ locations: [], diffs: [] });
  expect(Cursor.toolFileRefs("ApplyPatch", 42)).toEqual({ locations: [], diffs: [] });
  // non-object args to a regular tool
  expect(Cursor.toolFileRefs("Read", "raw")).toEqual({ locations: [], diffs: [] });
  // hunk tools without a path record nothing
  expect(Cursor.toolFileRefs("StrReplace", { old_string: "a" }).diffs).toEqual([]);
  // one-sided and string-free calls stay honest about what was recorded
  expect(Cursor.toolFileRefs("StrReplace", { path: "/w/a.ts", new_string: "n" }).diffs).toEqual([
    { path: "/w/a.ts", newText: "n" },
  ]);
  expect(Cursor.toolFileRefs("StrReplace", { path: "/w/a.ts" }).diffs).toEqual([]);
  // Write tolerates both `contents` and `content`, but needs one of them
  expect(Cursor.toolFileRefs("Write", { path: "/w/b.ts", content: "x" }).diffs).toEqual([
    { path: "/w/b.ts", newText: "x" },
  ]);
  expect(Cursor.toolFileRefs("Write", { path: "/w/b.ts" }).diffs).toEqual([]);
  // path lists contribute locations; junk entries drop
  expect(
    Cursor.toolFileRefs("SemanticSearch", { target_directories: ["/w", "", 42] }).locations,
  ).toEqual([{ path: "/w" }]);
});

test("store tool calls carry locations and diffs from args", () => {
  const session = Cursor.sessionFromStore({
    id: "chat-2",
    blobs: new Map<string, Uint8Array>([
      [blobId(0), checkpoint([blobId(1)])],
      [
        blobId(1),
        json({
          role: "assistant",
          content: [
            {
              type: "tool-call",
              toolCallId: "tool_edit",
              toolName: "StrReplace",
              args: { path: "/w/a.ts", old_string: "o", new_string: "n" },
            },
            {
              type: "tool-call",
              toolCallId: "tool_patch",
              toolName: "ApplyPatch",
              args: {
                patch: "*** Begin Patch\n*** Update File: /w/b.ts\n@@\n-a\n+b\n*** End Patch",
              },
            },
            {
              type: "tool-call",
              toolCallId: "tool_read",
              toolName: "ReadLints",
              args: { paths: ["/w/x.ts", "/w/y.ts"] },
            },
          ],
        }),
      ],
    ]),
  });
  const calls = session.nodes[0].toolCalls;
  expect(calls[0].diffs).toEqual([{ path: "/w/a.ts", oldText: "o", newText: "n" }]);
  // object-shaped patch args decode through the same parser
  expect(calls[1].diffs).toEqual([{ path: "/w/b.ts", oldText: "a", newText: "b" }]);
  expect(calls[2].locations).toEqual([{ path: "/w/x.ts" }, { path: "/w/y.ts" }]);
  expect(calls[2].diffs).toEqual([]);
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
  meta: string | undefined;
  readonly blobs: Map<string, Uint8Array>;
}

const fakeStoreDb = (
  stores: Map<string, FakeStore>,
  create = false,
): CursorRepository.OpenStoreDb => {
  return (path) => {
    let store = stores.get(path);
    if (store === undefined) {
      if (!create) {
        return Effect.fail(new StorageError({ message: `no store at ${path}` }));
      }
      // sqlite creates the file on a writable open — mirror that so the
      // chat scan sees `store.db` in the dir listing.
      store = { meta: undefined, blobs: new Map() };
      stores.set(path, store);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, "");
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
      run: (sql, params = []) => {
        const lower = sql.toLowerCase();
        if (lower.includes("into blobs")) {
          const [id, data] = params as [string, Uint8Array];
          if (lower.includes("or ignore") && store.blobs.has(id)) return;
          store.blobs.set(id, data);
          return;
        }
        if (lower.includes("into meta")) {
          store.meta = params[0] as string;
        }
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

test("repository hasSession and unsafe write ids", async () => {
  const { repo } = fixture();
  await expect(Effect.runPromise(repo.hasSession("chat-1"))).resolves.toBe(true);
  await expect(Effect.runPromise(repo.hasSession("sub-1"))).resolves.toBe(true);
  await expect(Effect.runPromise(repo.hasSession("nope"))).resolves.toBe(false);

  const bad = Session.make({
    id: "../escape",
    title: "x",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 1,
    mainChainId: 0,
    metadata: null,
  });
  await expect(Effect.runPromise(repo.save(bad))).rejects.toThrow(/safe file name/);
  await expect(Effect.runPromise(repo.delete("../x"))).rejects.toThrow(/safe file name/);
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

/* ------------------------------------------------------------- */
/* write path — agent-transcripts projection                      */
/* ------------------------------------------------------------- */

test("projectSlugFromCwd flattens separators like the real layout", () => {
  expect(Cursor.projectSlugFromCwd("/home/luis/Desktop/cheloni-v4")).toBe(
    "home-luis-Desktop-cheloni-v4",
  );
  expect(Cursor.projectSlugFromCwd("/tmp/0b0ce061-35f7")).toBe("tmp-0b0ce061-35f7");
  // every non-alphanumeric flattens, including dots and spaces
  expect(Cursor.projectSlugFromCwd("/w/my proj.v2")).toBe("w-my-proj-v2");
  expect(Cursor.projectSlugFromCwd("/")).toBe("root");
});

test("toTranscriptJsonl encodes only what the projection carries", () => {
  const session = Session.make({
    id: "new-chat",
    title: "title lives nowhere in the projection",
    workingDirectory: "/w",
    model: "composer-1",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_300,
    mainChainId: 3,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "system",
        content: "sys",
        createdAt: 1_700_000_000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        role: "user",
        content: "explore the repo",
        createdAt: 1_700_000_001,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 2,
        role: "assistant",
        content: "I'll look around.",
        thinking: Option.some(Cursor.REDACTED_THINKING),
        toolCalls: [
          ToolCall.make({ id: "call-1", name: "Glob", arguments: { glob_pattern: "src/**" } }),
        ],
        createdAt: 1_700_000_002,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 3,
        role: "tool",
        content: "result text",
        createdAt: 1_700_000_003,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 4,
        role: "user",
        content: "",
        createdAt: 1_700_000_004,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 5,
        role: "assistant",
        content: "",
        createdAt: 1_700_000_005,
        metadata: null,
      }),
    ],
  });

  const lines = Cursor.toTranscriptJsonl(session).trim().split("\n");
  // system, tool-result and empty nodes have no slot in the projection
  expect(lines).toHaveLength(2);
  expect(JSON.parse(lines[0])).toEqual({
    role: "user",
    message: { content: [{ type: "text", text: "explore the repo" }] },
  });
  expect(JSON.parse(lines[1])).toEqual({
    role: "assistant",
    message: {
      content: [
        { type: "text", text: "I'll look around.\n\n[REDACTED]" },
        { type: "tool_use", id: "call-1", name: "Glob", input: { glob_pattern: "src/**" } },
      ],
    },
  });
});

test("toTranscriptJsonl marks thinking even without visible text", () => {
  const session = Session.make({
    id: "s",
    title: "t",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 1,
    mainChainId: 0,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "assistant",
        content: "",
        thinking: Option.some("hidden reasoning"),
        createdAt: 1,
        metadata: null,
      }),
    ],
  });
  expect(JSON.parse(Cursor.toTranscriptJsonl(session).trim())).toEqual({
    role: "assistant",
    message: { content: [{ type: "text", text: "[REDACTED]" }] },
  });
});

const writableNodes = (): ReadonlyArray<MessageNode> => [
  MessageNode.make({
    nodeId: 0,
    role: "user",
    content: "explore the repo",
    createdAt: 1_700_000_000,
    metadata: null,
  }),
  MessageNode.make({
    nodeId: 1,
    parentNodeId: Option.some(0),
    role: "assistant",
    content: "I'll look around.",
    thinking: Option.some(Cursor.REDACTED_THINKING),
    toolCalls: [
      ToolCall.make({ id: "call-1", name: "Glob", arguments: { glob_pattern: "src/**" } }),
    ],
    createdAt: 1_700_000_100,
    metadata: null,
  }),
];

const writableSession = (
  id: string,
  cwd: string,
  overrides: {
    readonly metadata?: unknown;
    readonly parentSessionId?: Option.Option<string>;
    readonly nodes?: ReadonlyArray<MessageNode>;
  } = {},
): Session =>
  Session.make({
    id,
    title: "unused title",
    workingDirectory: cwd,
    model: "composer-1",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_300,
    mainChainId: 1,
    metadata: overrides.metadata ?? { source: "import" },
    parentSessionId: overrides.parentSessionId ?? Option.none(),
    nodes: overrides.nodes ?? writableNodes(),
    promptHistory: [],
  });

const writableRepo = (
  root: string,
  stores: Map<string, FakeStore> = new Map(),
): { readonly repo: ReturnType<typeof CursorRepository.makeCursorSessionRepository> } => ({
  repo: CursorRepository.makeCursorSessionRepository({
    cursorDir: root,
    openStoreDb: fakeStoreDb(stores),
    openWritableStoreDb: fakeStoreDb(stores, true),
  }),
});

test("save writes a canonical store plus the transcript and round-trips", async () => {
  const root = makeTree({});
  const { repo } = writableRepo(root);
  const session = writableSession("new-chat", "/home/luis/Desktop/cheloni");

  await Effect.runPromise(repo.save(session));

  // the canonical store: chats/<md5(cwd)>/<id>/ + sidecars
  const wsHash = Cursor.workspaceHashFromCwd("/home/luis/Desktop/cheloni");
  const chatDir = join(root, "chats", wsHash, "new-chat");
  expect(existsSync(join(chatDir, "store.db"))).toBe(true);
  expect(JSON.parse(readFileSync(join(chatDir, "meta.json"), "utf8"))).toEqual({
    schemaVersion: 1,
    createdAtMs: 1_700_000_000_000,
    hasConversation: true,
    title: "unused title",
    updatedAtMs: 1_700_000_300_000,
    cwd: "/home/luis/Desktop/cheloni",
  });
  expect(JSON.parse(readFileSync(join(chatDir, "prompt_history.json"), "utf8"))).toEqual([
    "explore the repo",
  ]);

  // the transcript projection lands alongside, like Cursor's own write
  const filePath = join(
    root,
    "projects/home-luis-Desktop-cheloni/agent-transcripts/new-chat/new-chat.jsonl",
  );
  expect(existsSync(filePath)).toBe(true);
  const lines = readFileSync(filePath, "utf8").trim().split("\n");
  expect(JSON.parse(lines[1]).message.content[1]).toEqual({
    type: "tool_use",
    id: "call-1",
    name: "Glob",
    input: { glob_pattern: "src/**" },
  });

  expect(await Effect.runPromise(repo.hasSession("new-chat"))).toBe(true);
  const found = await Effect.runPromise(repo.getById("new-chat"));
  const read = Option.getOrThrow(found);
  // the chat store wins over the projection and carries the richer meta
  expect(read.title).toBe("unused title");
  expect((read.metadata as Record<string, unknown>).store).toBe("chats");
  expect(read.workingDirectory).toBe("/home/luis/Desktop/cheloni");
  expect(read.lastActivityAt).toBe(1_700_000_300);
  expect(read.promptHistory.map((p) => p.content)).toEqual(["explore the repo"]);

  const [user, assistant] = read.nodes;
  expect(read.nodes).toHaveLength(2);
  expect(user.role).toBe("user");
  expect(user.content).toBe("explore the repo");
  expect(assistant.content).toBe("I'll look around.");
  expect(Option.getOrUndefined(assistant.thinking)).toBe(Cursor.REDACTED_THINKING);
  expect(assistant.toolCalls[0]).toMatchObject({
    id: "call-1",
    name: "Glob",
    arguments: { glob_pattern: "src/**" },
  });
});

test("save fails cleanly when the store has no write surface", async () => {
  const root = makeTree({});
  const repo = CursorRepository.makeCursorSessionRepository({
    cursorDir: root,
    // a read-only store — `run` is absent, so save must refuse cleanly
    openStoreDb: () =>
      Effect.succeed({
        all: () => [],
        close: () => {},
      }),
  });
  await expect(Effect.runPromise(repo.save(writableSession("c1", "/w")))).rejects.toThrow(
    /write surface/,
  );
});

test("save reuses the recorded project slug and overwrites cleanly", async () => {
  const root = makeTree({});
  const { repo } = writableRepo(root);
  const session = writableSession("chat-x", "/unrelated/cwd", {
    metadata: { source: "cursor", store: "transcript", project: "home-luis-Desktop" },
  });
  await Effect.runPromise(repo.save(session));
  expect(
    existsSync(join(root, "projects/home-luis-Desktop/agent-transcripts/chat-x/chat-x.jsonl")),
  ).toBe(true);

  // a recorded slug that isn't a safe file name falls back to the cwd slug
  const unsafe = writableSession("chat-y", "/w/fallback", {
    metadata: { store: "transcript", project: "../escape" },
  });
  await Effect.runPromise(repo.save(unsafe));
  expect(existsSync(join(root, "projects/w-fallback/agent-transcripts/chat-y/chat-y.jsonl"))).toBe(
    true,
  );

  // a second save replaces the file wholesale
  const updated = writableSession("chat-x", "/unrelated/cwd", {
    metadata: { store: "transcript", project: "home-luis-Desktop" },
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "replacement",
        createdAt: 1_700_000_000,
        metadata: null,
      }),
    ],
  });
  await Effect.runPromise(repo.save(updated));
  const read = Option.getOrThrow(await Effect.runPromise(repo.getById("chat-x")));
  expect(read.nodes.map((n) => n.content)).toEqual(["replacement"]);
});

test("save writes subagents under the parent's transcript dir", async () => {
  const root = makeTree({});
  const { repo } = writableRepo(root);
  // the parent lives in a different project than the child's cwd slug
  await Effect.runPromise(repo.save(writableSession("parent-1", "/elsewhere/proj")));

  const child = writableSession("sub-9", "/home/luis/Desktop", {
    parentSessionId: Option.some("parent-1"),
  });
  await Effect.runPromise(repo.save(child));

  const filePath = join(
    root,
    "projects/elsewhere-proj/agent-transcripts/parent-1/subagents/sub-9.jsonl",
  );
  expect(existsSync(filePath)).toBe(true);

  const read = Option.getOrThrow(await Effect.runPromise(repo.getById("sub-9")));
  expect(Option.getOrUndefined(read.parentSessionId)).toBe("parent-1");
  expect(read.workingDirectory).toBe("/elsewhere/proj");
});

test("save a subagent without a parent on disk still lands under it", async () => {
  const root = makeTree({});
  const { repo } = writableRepo(root);
  const child = writableSession("sub-1", "/w/proj", {
    parentSessionId: Option.some("ghost-parent"),
  });
  await Effect.runPromise(repo.save(child));
  expect(
    existsSync(join(root, "projects/w-proj/agent-transcripts/ghost-parent/subagents/sub-1.jsonl")),
  ).toBe(true);
  const read = Option.getOrThrow(await Effect.runPromise(repo.getById("sub-1")));
  expect(Option.getOrUndefined(read.parentSessionId)).toBe("ghost-parent");
});

test("save updates an existing chats store.db in place", async () => {
  const { root } = fixture();
  // a writable opener over the fixture's real store map — save must land
  // in the existing chats/wsHASH/chat-1 dir, not a fresh md5(cwd) one
  const stores = layout(root);
  const repo = CursorRepository.makeCursorSessionRepository({
    cursorDir: root,
    openStoreDb: fakeStoreDb(stores),
    openWritableStoreDb: fakeStoreDb(stores, true),
  });
  await Effect.runPromise(repo.save(writableSession("chat-1", "/different/cwd")));

  const store = stores.get(join(root, "chats/wsHASH/chat-1/store.db"))!;
  const meta = Cursor.parseStoreMeta(store.meta);
  expect(meta?.agentId).toBe("chat-1");
  expect(meta?.latestRootBlobId).not.toBe(blobId(0));
  // the original creation stamp survives the rewrite
  expect(meta?.createdAt).toBe(1_700_000_000_000);

  // the new root checkpoint lists the session's two message blobs
  const checkpointData = store.blobs.get(meta!.latestRootBlobId!);
  const decoded = Cursor.decodeCheckpoint(checkpointData!);
  expect(decoded?.messageIds).toHaveLength(2);
  // additive write — the store's earlier DAG entries are kept
  expect(store.blobs.has(blobId(1))).toBe(true);
  expect(store.blobs.has(blobId(0))).toBe(true);

  // and the whole record reads back through the store path
  const read = Option.getOrThrow(await Effect.runPromise(repo.getById("chat-1")));
  expect((read.metadata as Record<string, unknown>).store).toBe("chats");
  expect(read.nodes.map((n) => n.role)).toEqual(["user", "assistant"]);
});

/* ------------------------------------------------------------- */
/* write path — store.db encoders                                  */
/* ------------------------------------------------------------- */

test("blob ids and workspace hashes match the real content addressing", () => {
  // sha256("") is the empty blob every real store keeps
  expect(Cursor.blobIdFor(new Uint8Array(0))).toBe(
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  );
  // md5 of the raw path — both values observed on a live ~/.cursor/chats
  expect(Cursor.workspaceHashFromCwd("/home/luis")).toBe("bd2ea8ea8fb4de9e940176cfdc5cd7bd");
  expect(Cursor.workspaceHashFromCwd("/home/luis/Desktop")).toBe(
    "e4f40720b3eb141b3f86d6db5f82b915",
  );
  // the file:// uri encodes each segment and decodes back to the path
  const uri = Cursor.workspaceUriFromCwd("/home/luis/My Proj");
  expect(uri).toBe("file:///home/luis/My%20Proj");
  expect(Cursor.workspaceFromUri(uri)).toBe("/home/luis/My Proj");
  expect(Cursor.workspaceUriFromCwd("/home/luis/Desktop")).toBe("file:///home/luis/Desktop");
});

test("encodeCheckpoint round-trips through decodeCheckpoint", () => {
  const blob = Cursor.encodeCheckpoint({
    messageIds: [blobId(1), blobId(2)],
    workspace: "file:///home/luis/proj",
  });
  expect(Cursor.decodeCheckpoint(blob)).toEqual({
    messageIds: [blobId(1), blobId(2)],
    workspace: "file:///home/luis/proj",
    client: "cli",
  });
  // malformed refs are skipped; a bare checkpoint still records the client
  const bare = Cursor.encodeCheckpoint({ messageIds: ["not-hex", blobId(3)], client: "x" });
  expect(Cursor.decodeCheckpoint(bare)).toEqual({
    messageIds: [blobId(3)],
    workspace: undefined,
    client: "x",
  });
});

test("encodeStoreMeta and encodeMetaJson round-trip through their parsers", () => {
  const meta = Cursor.parseStoreMeta(
    Cursor.encodeStoreMeta({
      agentId: "chat-1",
      latestRootBlobId: blobId(7),
      name: "T",
      mode: "default",
      isRunEverything: false,
      createdAt: 1_700_000_000_000,
      lastUsedModel: "default",
    }),
  );
  expect(meta).toEqual({
    agentId: "chat-1",
    latestRootBlobId: blobId(7),
    name: "T",
    mode: "default",
    isRunEverything: false,
    createdAt: 1_700_000_000_000,
    lastUsedModel: "default",
  });

  const sidecar = Cursor.parseMetaJson(
    Cursor.encodeMetaJson({
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_000_300_000,
      title: "T",
      hasConversation: true,
      cwd: "/w",
    }),
  );
  expect(sidecar).toEqual({
    schemaVersion: 1,
    createdAtMs: 1_700_000_000_000,
    updatedAtMs: 1_700_000_300_000,
    title: "T",
    hasConversation: true,
    cwd: "/w",
  });
  // optional fields stay absent rather than serialising as nulls
  expect(Cursor.parseMetaJson(Cursor.encodeMetaJson({
    createdAtMs: 1,
    updatedAtMs: 2,
    hasConversation: false,
  }))).toEqual({
    schemaVersion: 1,
    createdAtMs: 1,
    updatedAtMs: 2,
    title: undefined,
    hasConversation: false,
    cwd: undefined,
  });
});

test("messageBlobsFromSession emits the shapes the reader decodes", () => {
  const session = Session.make({
    id: "s1",
    title: "t",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 1,
    mainChainId: 5,
    metadata: null,
    nodes: [
      MessageNode.make({ nodeId: 0, role: "system", content: "sys", createdAt: 1, metadata: null }),
      MessageNode.make({
        nodeId: 1,
        role: "user",
        content: "<user_info>\nOS: linux\n</user_info>",
        createdAt: 1,
        metadata: { context: "user_info" },
      }),
      MessageNode.make({
        nodeId: 2,
        role: "user",
        content: "<user_query>\ndo it\n</user_query>",
        requestId: Option.some("req-9"),
        createdAt: 1,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 3,
        role: "assistant",
        content: "on it",
        thinking: Option.some(Cursor.REDACTED_THINKING),
        thinkingSignature: Option.some("sealed-data"),
        toolCalls: [ToolCall.make({ id: "t1", name: "Shell", arguments: { command: "ls" } })],
        createdAt: 1,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 4,
        role: "tool",
        content: "ok",
        toolCallId: Option.some("t1"),
        toolName: Option.some("Shell"),
        toolResult: Option.some({ status: "success" as const, durationMs: 42 }),
        createdAt: 1,
        metadata: null,
      }),
      // nodes with nothing a blob can hold are skipped
      MessageNode.make({ nodeId: 5, role: "user", content: "", createdAt: 1, metadata: null }),
      MessageNode.make({ nodeId: 6, role: "assistant", content: "", createdAt: 1, metadata: null }),
    ],
  });

  const blobs = Cursor.messageBlobsFromSession(session);
  expect(blobs).toHaveLength(5);
  for (const blob of blobs) {
    expect(Cursor.blobIdFor(blob.data)).toBe(blob.id);
  }
  const parsed = blobs.map((b) => JSON.parse(new TextDecoder().decode(b.data)));

  expect(parsed[0]).toEqual({ role: "system", content: "sys" });
  // user_info context stays the raw string form
  expect(parsed[1]).toEqual({ role: "user", content: "<user_info>\nOS: linux\n</user_info>" });
  // a query keeps its recorded requestId
  expect(parsed[2]).toEqual({
    role: "user",
    content: [{ type: "text", text: "<user_query>\ndo it\n</user_query>" }],
    providerOptions: { cursor: { requestId: "req-9" } },
  });
  expect(parsed[3]).toEqual({
    role: "assistant",
    id: "1",
    content: [
      { type: "redacted-reasoning", data: "sealed-data" },
      { type: "text", text: "on it" },
      { type: "tool-call", toolCallId: "t1", toolName: "Shell", args: { command: "ls" } },
    ],
  });
  expect(parsed[4]).toEqual({
    role: "tool",
    id: "t1",
    content: [{ type: "tool-result", toolCallId: "t1", toolName: "Shell", result: "ok" }],
    providerOptions: {
      cursor: { highLevelToolCallResult: { output: { isError: false, success: { executionTime: 42 } } } },
    },
  });
});

test("messageBlobsFromSession regroups tool results by their blobId", () => {
  const shared = blobId(99);
  const toolNode = (nodeId: number, callId: string, result: string, blobId?: string) =>
    MessageNode.make({
      nodeId,
      role: "tool",
      content: result,
      toolCallId: Option.some(callId),
      toolName: Option.some("Shell"),
      createdAt: 1,
      metadata: blobId === undefined ? null : { blobId },
    });
  const session = Session.make({
    id: "s",
    title: "t",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 1,
    mainChainId: 3,
    metadata: null,
    nodes: [
      toolNode(0, "t1", "first", shared),
      toolNode(1, "t2", "second", shared),
      toolNode(2, "t3", "ungrouped"),
      toolNode(3, "t4", "other", blobId(77)),
    ],
  });
  const blobs = Cursor.messageBlobsFromSession(session);
  expect(blobs).toHaveLength(3);
  const grouped = JSON.parse(new TextDecoder().decode(blobs[0].data));
  expect(grouped.content).toHaveLength(2);
  expect(grouped.id).toBe("t1");
  // the unrecorded node gets its own blob
  expect(JSON.parse(new TextDecoder().decode(blobs[1].data)).content).toHaveLength(1);
});

test("storeWritePlan roots an empty session at the empty blob", () => {
  const empty = Session.make({
    id: "fresh",
    title: "",
    workingDirectory: "/w",
    model: "unknown",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_000,
    mainChainId: 0,
    agentMode: "",
    metadata: null,
    nodes: [],
  });
  const plan = Cursor.storeWritePlan(empty);
  expect(plan.rootBlobId).toBe(Cursor.blobIdFor(new Uint8Array(0)));
  const meta = Cursor.parseStoreMeta(plan.metaRow);
  expect(meta?.latestRootBlobId).toBe(plan.rootBlobId);
  expect(meta?.lastUsedModel).toBe("default");
  expect(meta?.name).toBeUndefined();
  expect(plan.promptHistoryJson).toBeUndefined();
});

test("storeWritePlan preserves a prior createdAt and records cursor meta", () => {
  const session = writableSession("chat-z", "/w", {
    metadata: { mode: "ask", isRunEverything: true },
  });
  const plan = Cursor.storeWritePlan(session, { createdAtMs: 1_600_000_000_000 });
  const meta = Cursor.parseStoreMeta(plan.metaRow);
  expect(meta?.createdAt).toBe(1_600_000_000_000);
  expect(meta?.mode).toBe("ask");
  expect(meta?.isRunEverything).toBe(true);
  expect(meta?.lastUsedModel).toBe("composer-1");
  // checkpoint ids all resolve to real blobs in the plan
  const checkpoint = plan.blobs[plan.blobs.length - 1];
  expect(checkpoint.id).toBe(plan.rootBlobId);
  const decoded = Cursor.decodeCheckpoint(checkpoint.data);
  expect(decoded?.workspace).toBe("file:///w");
  for (const id of decoded?.messageIds ?? []) {
    expect(plan.blobs.some((b) => b.id === id)).toBe(true);
  }
});

test("delete removes transcript dirs, subagent files and chat stores", async () => {
  const { repo, root } = fixture();

  await Effect.runPromise(repo.delete("sub-1"));
  expect(
    existsSync(
      join(root, "projects/home-luis-Desktop/agent-transcripts/chat-2/subagents/sub-1.jsonl"),
    ),
  ).toBe(false);
  expect(await Effect.runPromise(repo.hasSession("sub-1"))).toBe(false);
  // the parent chat survives its subagent's removal
  expect(await Effect.runPromise(repo.hasSession("chat-2"))).toBe(true);

  // deleting a chat removes both its store and its transcript (with the
  // remaining subagents, which live inside the chat's dir)
  await Effect.runPromise(repo.delete("chat-1"));
  expect(existsSync(join(root, "chats/wsHASH/chat-1"))).toBe(false);
  expect(existsSync(join(root, "projects/home-luis-Desktop/agent-transcripts/chat-1"))).toBe(false);
  expect(await Effect.runPromise(repo.hasSession("chat-1"))).toBe(false);

  // deleting an unknown id is a no-op
  await Effect.runPromise(repo.delete("ghost"));
});
