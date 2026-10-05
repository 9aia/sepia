/**
 * Cursor.ts edge coverage — protobuf/JSON decode rejections, blob-message
 * fallback shapes, transcript projection skips, and the writer-side
 * optional-field branches `storeWritePlan`/`encode*` carry.
 */
import { Option } from "effect";
import { expect, test } from "vite-plus/test";
import * as Cursor from "../src/Cursor.js";
import { MessageNode, Session, ToolCall } from "../src/Domain.js";

const enc = new TextEncoder();
const bytes = (text: string): Uint8Array => enc.encode(text);
const json = (value: unknown): Uint8Array => bytes(JSON.stringify(value));

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

const sessionOf = (nodes: ReadonlyArray<MessageNode>, over = {}): Session =>
  Session.make({
    id: "chat-1",
    title: "t",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 2,
    mainChainId: nodes.length - 1,
    metadata: null,
    nodes,
    ...over,
  });

const node = (over: Partial<Parameters<typeof MessageNode.make>[0]>): MessageNode =>
  MessageNode.make({
    nodeId: 0,
    role: "user",
    content: "hi",
    createdAt: 1,
    metadata: null,
    ...over,
  });

/* ---- scalar decoders --------------------------------------------------- */

test("parseStoreMeta and parseMetaJson reject non-object JSON", () => {
  // "35" is hex for "5" — valid JSON, not an object
  expect(Cursor.parseStoreMeta("35")).toBeUndefined();
  expect(Cursor.parseMetaJson("5")).toBeUndefined();
  expect(Cursor.parseMetaJson("{bad")).toBeUndefined();
});

test("decodeCheckpoint rejects truncated fields and skips unknown wire types", () => {
  // field 0 is never a real field
  expect(Cursor.decodeCheckpoint(new Uint8Array([0x00]))).toBeUndefined();
  // a varint field value that never terminates
  expect(Cursor.decodeCheckpoint(new Uint8Array([0x08, 0x80]))).toBeUndefined();
  // a length-delimited field whose varint is truncated
  expect(Cursor.decodeCheckpoint(new Uint8Array([0x0a, 0x80]))).toBeUndefined();
  // a declared length that overruns the buffer
  expect(Cursor.decodeCheckpoint(new Uint8Array([0x0a, 0x05, 0x01]))).toBeUndefined();
  // a 32-bit field with fewer than 4 bytes left
  expect(Cursor.decodeCheckpoint(new Uint8Array([0x0d, 0x01, 0x02]))).toBeUndefined();
  // a group wire type the walker does not handle
  expect(Cursor.decodeCheckpoint(new Uint8Array([0x0b]))).toBeUndefined();
  // a valid 64-bit field plus a real message ref: skipped, ref decoded
  const data = concat(new Uint8Array([0x09, 1, 2, 3, 4, 5, 6, 7, 8]), field(1, idBytes(blobId(3))));
  expect(Cursor.decodeCheckpoint(data)).toEqual({
    messageIds: [blobId(3)],
    workspace: undefined,
    client: undefined,
  });
});

test("workspaceFromUri rejects non-file uris and bad percent-encoding", () => {
  expect(Cursor.workspaceFromUri(undefined)).toBeUndefined();
  expect(Cursor.workspaceFromUri("https://x")).toBeUndefined();
  expect(Cursor.workspaceFromUri("file:///%zz")).toBeUndefined();
  expect(Cursor.workspaceFromUri("file://")).toBeUndefined();
  expect(Cursor.workspaceFromUri("file:///w")).toBe("/w");
});

/* ---- blob message fallbacks -------------------------------------------- */

test("sessionFromStore skips unparseable blobs and odd message shapes", () => {
  const blobs = new Map<string, Uint8Array>([
    [
      blobId(0),
      concat(
        field(1, idBytes(blobId(1))),
        field(1, idBytes(blobId(2))),
        field(1, idBytes(blobId(3))),
        field(1, idBytes(blobId(4))),
        field(1, idBytes(blobId(5))),
        field(1, idBytes(blobId(6))),
        field(1, idBytes(blobId(7))),
        varintField(10, 1),
      ),
    ],
    // not a message at all — no role
    [blobId(1), json({ nope: true })],
    // user message with a scalar content — nothing readable
    [blobId(2), json({ role: "user", content: 42 })],
    // user message whose array has no text items
    [blobId(3), json({ role: "user", content: [{ type: "image" }, 5, null] })],
    // a tool message whose items are not tool-results → one fallback node
    [blobId(4), json({ role: "tool", content: [{ type: "x" }, 3] })],
    // a tool message with a result-less tool-result entry
    [
      blobId(5),
      json({
        role: "tool",
        content: [{ type: "tool-result", toolCallId: "c1", toolName: "Shell" }],
      }),
    ],
    // an unknown role becomes a system marker node
    [blobId(6), json({ role: "banana", content: "exotic" })],
    // a real user query so the session isn't empty
    [blobId(7), json({ role: "user", content: [{ type: "text", text: "real question" }] })],
  ]);
  const session = Cursor.sessionFromStore({
    id: "chat-1",
    meta: { agentId: "chat-1", latestRootBlobId: blobId(0), isRunEverything: false },
    blobs,
  });
  const roles = session.nodes.map((n) => n.role);
  expect(roles).toEqual(["tool", "tool", "system", "user"]);
  // the item-less tool blob fell back to stringifying its content array
  expect(session.nodes[0].content).toBe(JSON.stringify([{ type: "x" }, 3]));
  // the result-less tool-result still emits a node with empty output
  expect(session.nodes[1].content).toBe("");
  expect(session.nodes[2].content).toBe("[cursor banana]");
});

/* ---- transcript projection -------------------------------------------- */

const tl = (entry: Record<string, unknown>): string => JSON.stringify(entry);

test("fromTranscriptJsonl skips empty users, junk items and empty assistants", () => {
  const session = Cursor.fromTranscriptJsonl(
    [
      tl({ role: "user", message: { content: "" } }),
      tl({ role: "user", message: { content: [{ type: "text", text: "  " }] } }),
      // markup-only text extracts no query — dropped from history entirely
      tl({ role: "user", message: { content: [{ type: "text", text: "<br>" }] } }),
      tl({ role: "user", message: { content: [{ type: "text", text: "real" }] } }),
      // assistant items that are not objects are skipped
      tl({
        role: "assistant",
        message: { content: ["junk", { type: "text", text: "plain text" }] },
      }),
      // an assistant entry with nothing usable emits no node
      tl({ role: "assistant", message: { content: [{ type: "image" }] } }),
      tl({ role: "assistant", message: { content: "not an array" } }),
      // a turn-level error is recorded on the session metadata
      tl({ type: "turn_ended", status: "error", error: { message: "boom" } }),
    ].join("\n"),
    { id: "t1", projectSlug: "home-x", mtimeMs: 1_700_000_000_000 },
  );
  const roles = session.nodes.map((n) => n.role);
  // whitespace/markup-only user text still emits a node — the skip only
  // applies to prompt history, where no query can be extracted
  expect(roles).toEqual(["user", "user", "user", "assistant"]);
  expect(session.nodes.map((n) => n.content)).toEqual(["  ", "<br>", "real", "plain text"]);
  expect(session.promptHistory.map((p) => p.content)).toEqual(["real"]);
  expect((session.metadata as Record<string, unknown>).turnErrors).toEqual([{ message: "boom" }]);
});

/* ---- writers ------------------------------------------------------------ */

test("encodeCheckpoint skips unusable message refs and omits absent fields", () => {
  const data = Cursor.encodeCheckpoint({
    messageIds: ["not-hex", "abcd", blobId(7)],
    // no workspace, no client → fields 9 and default 22
  });
  const decoded = Cursor.decodeCheckpoint(data);
  expect(decoded?.messageIds).toEqual([blobId(7)]);
  expect(decoded?.workspace).toBeUndefined();
  expect(decoded?.client).toBe("cli");
});

test("encodeStoreMeta and encodeMetaJson omit absent fields", () => {
  const meta = Cursor.parseStoreMeta(
    Cursor.encodeStoreMeta({ agentId: "a", latestRootBlobId: blobId(1) }),
  );
  expect(meta).toMatchObject({ agentId: "a", latestRootBlobId: blobId(1) });
  expect(meta?.name).toBeUndefined();
  expect(meta?.mode).toBeUndefined();
  expect(meta?.createdAt).toBeUndefined();
  expect(meta?.lastUsedModel).toBeUndefined();

  const metaJson = JSON.parse(
    Cursor.encodeMetaJson({ createdAtMs: 1, updatedAtMs: 2, hasConversation: false, title: "" }),
  );
  expect(metaJson.title).toBeUndefined();
  expect(metaJson.cwd).toBeUndefined();
  expect(metaJson.hasConversation).toBe(false);
});

test("messageBlobsFromSession handles seal-less thinking, id-less tools and empty nodes", () => {
  const blobs = Cursor.messageBlobsFromSession(
    sessionOf([
      // empty system and empty user emit no blob at all
      node({ nodeId: 0, role: "system", content: "" }),
      node({ nodeId: 1, role: "user", content: "" }),
      // user_info context rides as a raw string blob
      node({
        nodeId: 2,
        role: "user",
        content: "<user_info>x</user_info>",
        metadata: { context: "user_info" },
      }),
      // thinking without a seal still marks a reasoning block
      node({
        nodeId: 3,
        role: "assistant",
        content: "",
        thinking: Option.some("ponder"),
        toolCalls: [ToolCall.make({ id: "c1", name: "Shell", arguments: undefined })],
      }),
      // a tool node with no call linkage → no id/toolName on the result
      node({ nodeId: 4, role: "tool", content: "loose output" }),
    ]),
  );
  // messageJson returned undefined for the two empty nodes — no blobs for them
  const all = blobs.map((b) => JSON.parse(new TextDecoder().decode(b.data)));
  expect(all).toHaveLength(3);
  expect(all[0]).toEqual({ role: "user", content: "<user_info>x</user_info>" });
  expect(all[1].role).toBe("assistant");
  expect((all[1].content as Array<unknown>)[0]).toEqual({ type: "redacted-reasoning" });
  expect((all[1].content as Array<unknown>)[1]).toMatchObject({
    type: "tool-call",
    toolCallId: "c1",
    args: {},
  });
  expect(all[2]).toMatchObject({ role: "tool" });
  expect(all[2].id).toBeUndefined();
  expect((all[2].content as Array<Record<string, unknown>>)[0]).toEqual({
    type: "tool-result",
    result: "loose output",
  });
});

test("storeWritePlan derives prompts from nodes and defaults model/mode", () => {
  const plan = Cursor.storeWritePlan(
    sessionOf(
      [
        node({ nodeId: 0, role: "user", content: "first prompt" }),
        // markup-only text yields no query — excluded from prompt_history
        node({ nodeId: 1, role: "user", content: "<br>" }),
      ],
      { model: "", agentMode: "", metadata: { mode: 7, isRunEverything: "yes" } },
    ),
  );
  const metaJson = JSON.parse(plan.metaJson);
  expect(metaJson.hasConversation).toBe(true);
  const meta = Cursor.parseStoreMeta(plan.metaRow);
  expect(meta?.name).toBe("t");
  expect(meta?.mode).toBeUndefined();
  // a non-string mode field and "" agentMode resolve to nothing
  expect(JSON.parse(plan.promptHistoryJson!)).toEqual(["first prompt"]);

  // a session with no provable prompts writes no history file
  const empty = Cursor.storeWritePlan(sessionOf([], { title: "" }));
  expect(empty.promptHistoryJson).toBeUndefined();
});
