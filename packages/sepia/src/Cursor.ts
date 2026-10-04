import { Option } from "effect";
import {
  MessageNode,
  PromptHistoryEntry,
  REDACTED_THINKING,
  Session,
  ToolCall,
  type ToolResultInfo,
} from "./Domain.js";
import * as ClaudeCode from "./ClaudeCode.js";
import * as Devin from "./Devin.js";

/**
 * Cursor's agent CLI keeps chats under `~/.cursor/chats/<workspace-hash>/
 * <chat-id>/`: a content-addressed `store.db` (`blobs(id, data)` + a `meta`
 * KV table) plus `meta.json` and `prompt_history.json` sidecars. `meta['0']`
 * is hex-encoded JSON whose `latestRootBlobId` names a protobuf-ish
 * "checkpoint" blob; the checkpoint's repeated field-1 entries are the
 * 32-byte blob ids of the ordered message list. Message blobs are AI-SDK
 * JSON (`role` + `content`); all other binary blobs are UI/tool projections
 * sepia does not decode — they are counted as `opaqueBlobs` in metadata.
 *
 * A second, lossy projection lives at `~/.cursor/projects/<slug>/
 * agent-transcripts/<chat-id>/<chat-id>.jsonl` (plus `subagents/*.jsonl`):
 * one JSON message per line — `text` and `tool_use` blocks but no tool
 * results, usage, or timestamps — and `{"type":"turn_ended"}` markers.
 *
 * Reasoning is unrecoverable in both: `redacted-reasoning` blob payloads are
 * opaque and the transcript projects them as `[REDACTED]`; both map to the
 * `[redacted]` thinking marker. There is no writer and no ACP runtime —
 * the store is read-only.
 */

const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

const strField = (obj: Record<string, unknown>, key: string): string | undefined =>
  typeof obj[key] === "string" ? (obj[key] as string) : undefined;

const finiteNumber = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

const sanitize = (text: string | null | undefined): string => {
  if (!text) return "";
  let out = "";
  for (const ch of text) {
    const code = ch.charCodeAt(0);
    if (code >= 32 || code === 9 || code === 10 || code === 13) {
      out += ch;
    }
  }
  return out;
};

const HEX = /^[0-9a-fA-F]+$/;

const toHex = (bytes: Uint8Array): string => {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
};

const fromHex = (text: string): Uint8Array | undefined => {
  if (text.length % 2 !== 0 || !HEX.test(text)) return undefined;
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = Number.parseInt(text.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
};

const utf8 = (bytes: Uint8Array): string => new TextDecoder("utf-8").decode(bytes);

/** Marker recorded in `thinking` when a message carried opaque reasoning. */
export { REDACTED_THINKING };

/* ------------------------------------------------------------------ */
/* meta['0'] — hex-encoded JSON                                        */
/* ------------------------------------------------------------------ */

export interface CursorStoreMeta {
  readonly agentId?: string;
  readonly latestRootBlobId?: string;
  readonly name?: string;
  readonly mode?: string;
  readonly isRunEverything?: boolean;
  /** Epoch milliseconds. */
  readonly createdAt?: number;
  readonly lastUsedModel?: string;
}

/** `meta` rows are hex-encoded JSON; plain JSON is accepted too. */
export const parseStoreMeta = (value: unknown): CursorStoreMeta | undefined => {
  if (typeof value !== "string" || value === "") return undefined;
  const decoded = fromHex(value);
  const candidates = decoded === undefined ? [value] : [value, utf8(decoded)];
  for (const candidate of candidates) {
    try {
      const parsed: unknown = JSON.parse(candidate);
      if (!isObject(parsed)) continue;
      return {
        agentId: strField(parsed, "agentId"),
        latestRootBlobId: strField(parsed, "latestRootBlobId"),
        name: strField(parsed, "name"),
        mode: strField(parsed, "mode"),
        isRunEverything: parsed.isRunEverything === true,
        createdAt: finiteNumber(parsed.createdAt),
        lastUsedModel: strField(parsed, "lastUsedModel"),
      };
    } catch {
      // try the next encoding
    }
  }
  return undefined;
};

/* ------------------------------------------------------------------ */
/* meta.json sidecar                                                   */
/* ------------------------------------------------------------------ */

export interface CursorMetaJson {
  readonly schemaVersion?: number;
  readonly createdAtMs?: number;
  readonly updatedAtMs?: number;
  readonly title?: string;
  readonly hasConversation?: boolean;
  readonly cwd?: string;
}

export const parseMetaJson = (raw: string): CursorMetaJson | undefined => {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!isObject(parsed)) return undefined;
    return {
      schemaVersion: finiteNumber(parsed.schemaVersion),
      createdAtMs: finiteNumber(parsed.createdAtMs),
      updatedAtMs: finiteNumber(parsed.updatedAtMs),
      title: strField(parsed, "title"),
      hasConversation: parsed.hasConversation === true,
      cwd: strField(parsed, "cwd"),
    };
  } catch {
    return undefined;
  }
};

/** `prompt_history.json` is a flat JSON array of submitted prompt strings. */
export const parsePromptHistory = (raw: string): ReadonlyArray<string> => {
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
  }
};

/* ------------------------------------------------------------------ */
/* Protobuf-ish checkpoint blobs                                        */
/* ------------------------------------------------------------------ */

interface ProtoField {
  readonly field: number;
  readonly wire: number;
  readonly varint?: number;
  readonly data?: Uint8Array;
}

const readVarint = (buf: Uint8Array, pos: number): [number, number] | undefined => {
  let result = 0;
  let shift = 0;
  let i = pos;
  while (i < buf.length && shift < 64) {
    const b = buf[i++];
    result += (b & 0x7f) * 2 ** shift;
    if ((b & 0x80) === 0) return [result, i];
    shift += 7;
  }
  return undefined;
};

/** Lenient protobuf field walk; undefined when the buffer isn't field-shaped. */
const parseProtoFields = (buf: Uint8Array): ReadonlyArray<ProtoField> | undefined => {
  const fields: Array<ProtoField> = [];
  let pos = 0;
  while (pos < buf.length) {
    const key = readVarint(buf, pos);
    if (key === undefined) return undefined;
    pos = key[1];
    const field = Math.floor(key[0] / 8);
    const wire = key[0] % 8;
    if (field === 0) return undefined;
    if (wire === 0) {
      const v = readVarint(buf, pos);
      if (v === undefined) return undefined;
      pos = v[1];
      fields.push({ field, wire, varint: v[0] });
    } else if (wire === 2) {
      const len = readVarint(buf, pos);
      if (len === undefined) return undefined;
      pos = len[1];
      if (pos + len[0] > buf.length) return undefined;
      fields.push({ field, wire, data: buf.subarray(pos, pos + len[0]) });
      pos += len[0];
    } else if (wire === 5) {
      if (pos + 4 > buf.length) return undefined;
      fields.push({ field, wire });
      pos += 4;
    } else if (wire === 1) {
      if (pos + 8 > buf.length) return undefined;
      fields.push({ field, wire });
      pos += 8;
    } else {
      return undefined;
    }
  }
  return fields;
};

export interface CheckpointInfo {
  /** Ordered message-blob ids — the transcript prefix at this checkpoint. */
  readonly messageIds: ReadonlyArray<string>;
  /** `file://` workspace URI the chat ran in. */
  readonly workspace?: string;
  /** Client kind, e.g. `"cli"`. */
  readonly client?: string;
}

/**
 * A checkpoint blob: repeated field-1 length-32 entries are the content
 * hashes of the ordered messages; field 9 is the workspace URI and field 22
 * the client tag. Other fields (summary refs, UI state, counters) are not
 * decoded.
 */
export const decodeCheckpoint = (data: Uint8Array): CheckpointInfo | undefined => {
  const fields = parseProtoFields(data);
  if (fields === undefined) return undefined;
  const messageIds: Array<string> = [];
  let workspace: string | undefined;
  let client: string | undefined;
  for (const f of fields) {
    if (f.wire !== 2 || f.data === undefined) continue;
    if (f.field === 1 && f.data.length === 32) {
      messageIds.push(toHex(f.data));
    } else if (f.field === 9) {
      workspace = utf8(f.data);
    } else if (f.field === 22) {
      client = utf8(f.data);
    }
  }
  if (messageIds.length === 0 && workspace === undefined && client === undefined) {
    return undefined;
  }
  return { messageIds, workspace, client };
};

/** `file:///home/me/proj` → `/home/me/proj`; anything else is dropped. */
export const workspaceFromUri = (uri: string | undefined): string | undefined => {
  if (uri === undefined || !uri.startsWith("file://")) return undefined;
  try {
    const path = decodeURIComponent(uri.slice("file://".length));
    return path === "" ? undefined : path;
  } catch {
    return undefined;
  }
};

/* ------------------------------------------------------------------ */
/* store.db → IR                                                       */
/* ------------------------------------------------------------------ */

/** What a chat dir proves about the session before blobs are decoded. */
export interface CursorChatInfo {
  /** Chat id — the directory name under `chats/<workspace-hash>/`. */
  readonly id: string;
  /** The opaque workspace-hash dir the chat sits under (not decodable). */
  readonly workspaceHash?: string;
  /** Working directory when nothing in the store records one. */
  readonly fallbackCwd?: string;
}

export interface CursorStoreInput extends CursorChatInfo {
  readonly meta?: CursorStoreMeta;
  readonly metaJson?: CursorMetaJson;
  readonly blobs: ReadonlyMap<string, Uint8Array>;
  readonly promptHistory?: ReadonlyArray<string>;
}

interface BlobMessage {
  readonly role: string;
  readonly content: unknown;
  readonly providerOptions?: Record<string, unknown>;
}

const parseMessage = (data: Uint8Array): BlobMessage | undefined => {
  try {
    const parsed: unknown = JSON.parse(utf8(data));
    if (!isObject(parsed) || typeof parsed.role !== "string") return undefined;
    return {
      role: parsed.role,
      content: parsed.content,
      providerOptions: isObject(parsed.providerOptions) ? parsed.providerOptions : undefined,
    };
  } catch {
    return undefined;
  }
};

const cursorOptions = (msg: BlobMessage): Record<string, unknown> | undefined =>
  msg.providerOptions !== undefined && isObject(msg.providerOptions.cursor)
    ? msg.providerOptions.cursor
    : undefined;

/** Inner text of a `<user_query>…</user_query>` block, or the text itself. */
export const extractUserQuery = (text: string): string | undefined => {
  const match = /<user_query>([\s\S]*?)<\/user_query>/.exec(text);
  const inner = match === null ? undefined : match[1];
  const raw = inner ?? text.replace(/<[^>]+>/g, " ");
  const cleaned = raw.replace(/\s+/g, " ").trim();
  return cleaned === "" ? undefined : cleaned;
};

const isUserInfo = (content: unknown): boolean =>
  typeof content === "string" && content.trimStart().startsWith("<user_info>");

/**
 * `providerOptions.cursor.highLevelToolCallResult.output` carries the real
 * outcome — `isError` plus per-tool payloads (`success.executionTime` for
 * Shell). Non-Shell shapes keep whatever fields exist.
 */
const toolResultInfo = (result: Record<string, unknown>, msg: BlobMessage): ToolResultInfo => {
  const output = (() => {
    const cursor = cursorOptions(msg);
    const high = cursor === undefined ? undefined : cursor.highLevelToolCallResult;
    return isObject(high) && isObject(high.output) ? high.output : undefined;
  })();
  const isError = output?.isError === true || result.is_error === true;
  const success = output !== undefined && isObject(output.success) ? output.success : undefined;
  const durationMs = finiteNumber(success?.executionTime) ?? finiteNumber(output?.executionTime);
  return {
    status: isError ? "error" : "success",
    ...(durationMs === undefined ? {} : { durationMs }),
  };
};

const toolResultText = (result: unknown): string => {
  if (typeof result === "string") return result;
  if (result === undefined || result === null) return "";
  return JSON.stringify(result);
};

interface ToolPairing {
  readonly nameById: Map<string, string>;
  readonly argsById: Map<string, unknown>;
}

/** One decoded message blob → the IR node(s) it emits. */
const messageNodes = (
  msg: BlobMessage,
  blobId: string,
  nodeId: number,
  parent: Option.Option<number>,
  ts: number,
  pairing: ToolPairing,
): ReadonlyArray<MessageNode> => {
  const base = { nodeId, parentNodeId: parent, createdAt: ts };

  if (msg.role === "system") {
    return [
      MessageNode.make({
        ...base,
        role: "system",
        content: sanitize(
          typeof msg.content === "string" ? msg.content : toolResultText(msg.content),
        ),
        metadata: { blobId },
      }),
    ];
  }

  if (msg.role === "user") {
    if (typeof msg.content === "string") {
      return [
        MessageNode.make({
          ...base,
          role: "user",
          content: sanitize(msg.content),
          metadata: { blobId, ...(isUserInfo(msg.content) ? { context: "user_info" } : {}) },
        }),
      ];
    }
    if (!Array.isArray(msg.content)) return [];
    const textParts: Array<string> = [];
    for (const item of msg.content) {
      if (isObject(item) && item.type === "text" && typeof item.text === "string") {
        textParts.push(item.text);
      }
    }
    if (textParts.length === 0) return [];
    const requestId = cursorOptions(msg)?.requestId;
    return [
      MessageNode.make({
        ...base,
        role: "user",
        content: sanitize(textParts.join("\n")),
        requestId: typeof requestId === "string" ? Option.some(requestId) : Option.none(),
        metadata: { blobId },
      }),
    ];
  }

  if (msg.role === "assistant") {
    const textParts: Array<string> = [];
    const toolCalls: Array<ToolCall> = [];
    let redacted = false;
    // The `redacted-reasoning` payload is opaque; it rides the IR verbatim as
    // the thinking block's signature so a converted session keeps the seal.
    let redactedData: string | undefined;
    const content = Array.isArray(msg.content) ? msg.content : [];
    for (const item of content) {
      if (!isObject(item)) continue;
      if (item.type === "text" && typeof item.text === "string") {
        textParts.push(item.text);
        continue;
      }
      if (item.type === "redacted-reasoning") {
        redacted = true;
        const data = strField(item, "data");
        if (data !== undefined) redactedData = data;
        continue;
      }
      if (item.type === "tool-call") {
        const id = strField(item, "toolCallId") ?? `cursor-tool-${nodeId}-${toolCalls.length}`;
        const name = strField(item, "toolName") ?? "unknown";
        const args = item.args ?? item.input ?? {};
        toolCalls.push(ToolCall.make({ id, name, arguments: args, index: toolCalls.length }));
        pairing.nameById.set(id, name);
        pairing.argsById.set(id, args);
      }
    }
    return [
      MessageNode.make({
        ...base,
        role: "assistant",
        content: sanitize(textParts.join("\n")),
        thinking: redacted ? Option.some(REDACTED_THINKING) : Option.none(),
        thinkingSignature: Option.fromNullable(redactedData),
        toolCalls,
        metadata: { blobId, ...(redacted ? { redactedReasoning: true } : {}) },
      }),
    ];
  }

  if (msg.role === "tool") {
    const content = Array.isArray(msg.content) ? msg.content : [];
    const nodes: Array<MessageNode> = [];
    for (const item of content) {
      if (!isObject(item) || item.type !== "tool-result") continue;
      const callId = strField(item, "toolCallId");
      const resultName = strField(item, "toolName");
      nodes.push(
        MessageNode.make({
          nodeId: nodeId + nodes.length,
          parentNodeId: parent,
          role: "tool",
          content: sanitize(toolResultText(item.result)),
          toolCallId: callId === undefined ? Option.none() : Option.some(callId),
          toolName:
            resultName !== undefined
              ? Option.some(resultName)
              : callId === undefined
                ? Option.none()
                : Option.fromNullable(pairing.nameById.get(callId)),
          toolResult: Option.some(toolResultInfo(item, msg)),
          createdAt: ts,
          metadata: {
            blobId,
            toolArguments: callId === undefined ? null : (pairing.argsById.get(callId) ?? null),
          },
        }),
      );
    }
    if (nodes.length === 0) {
      nodes.push(
        MessageNode.make({
          ...base,
          role: "tool",
          content: sanitize(toolResultText(msg.content)),
          createdAt: ts,
          metadata: { blobId },
        }),
      );
    }
    return nodes;
  }

  return [
    MessageNode.make({
      ...base,
      role: "system",
      content: sanitize(`[cursor ${msg.role}]`),
      metadata: { blobId, role: msg.role },
    }),
  ];
};

const titleFrom = (title: string | undefined, firstUser: string | undefined, id: string): string =>
  title !== undefined && title !== ""
    ? title
    : firstUser !== undefined
      ? firstUser.slice(0, 80)
      : id;

/**
 * Full IR for a `chats/<ws>/<chat>/` store: the latest checkpoint's ordered
 * message list decoded into nodes. Binary blobs that are not message JSON
 * (checkpoints, prompt records, UI projections) are skipped and counted.
 */
export const sessionFromStore = (input: CursorStoreInput): Session => {
  const createdMs = input.metaJson?.createdAtMs ?? input.meta?.createdAt ?? Math.floor(Date.now());
  const ts = Math.floor(createdMs / 1000);

  const rootId = input.meta?.latestRootBlobId;
  const rootBlob = rootId === undefined ? undefined : input.blobs.get(rootId);
  let checkpoint =
    rootBlob !== undefined && rootBlob.length > 0 ? decodeCheckpoint(rootBlob) : undefined;
  if (checkpoint === undefined) {
    // No usable `latestRootBlobId` (missing/corrupt meta row): fall back to
    // the blob that decodes as the richest checkpoint.
    for (const data of input.blobs.values()) {
      if (data.length === 0) continue;
      const candidate = decodeCheckpoint(data);
      if (
        candidate !== undefined &&
        candidate.messageIds.length > (checkpoint?.messageIds.length ?? 0)
      ) {
        checkpoint = candidate;
      }
    }
  }
  const messageIds = checkpoint?.messageIds ?? [];
  const workspace = workspaceFromUri(checkpoint?.workspace);
  const client = checkpoint?.client;

  const nodes: Array<MessageNode> = [];
  const pairing: ToolPairing = { nameById: new Map(), argsById: new Map() };
  let firstUserText: string | undefined;
  let opaqueBlobs = 0;
  let lastNodeId: number | null = null;

  for (const id of messageIds) {
    const data = input.blobs.get(id);
    if (data === undefined || data.length === 0) {
      opaqueBlobs++;
      continue;
    }
    const msg = parseMessage(data);
    if (msg === undefined) {
      opaqueBlobs++;
      continue;
    }
    const parent = lastNodeId === null ? Option.none<number>() : Option.some(lastNodeId);
    const emitted = messageNodes(msg, id, nodes.length, parent, ts, pairing);
    for (const node of emitted) {
      // `<user_info>` context is environment plumbing, not a real prompt.
      const isContext = isObject(node.metadata) && node.metadata.context === "user_info";
      if (node.role === "user" && !isContext && firstUserText === undefined) {
        firstUserText = extractUserQuery(node.content);
      }
      nodes.push(node);
      lastNodeId = node.nodeId;
    }
  }

  const folded = Devin.applyToolCallOutcomes(nodes, Devin.toolNodeOutcomes(nodes));

  const promptHistory = (input.promptHistory ?? []).map((content) =>
    PromptHistoryEntry.make({ content: sanitize(content), timestamp: createdMs }),
  );

  const updatedMs = input.metaJson?.updatedAtMs ?? createdMs;
  return Session.make({
    id: input.id,
    title: titleFrom(input.metaJson?.title ?? input.meta?.name, firstUserText, input.id),
    workingDirectory: input.metaJson?.cwd ?? workspace ?? input.fallbackCwd ?? "/",
    backendType: "cursor",
    agentMode: input.meta?.mode ?? "default",
    model: input.meta?.lastUsedModel ?? "unknown",
    createdAt: ts,
    lastActivityAt: Math.max(ts, Math.floor(updatedMs / 1000)),
    mainChainId: folded.length > 0 ? folded[folded.length - 1].nodeId : 0,
    metadata: {
      source: "cursor",
      store: "chats",
      workspaceHash: input.workspaceHash ?? null,
      agentId: input.meta?.agentId ?? null,
      mode: input.meta?.mode ?? null,
      isRunEverything: input.meta?.isRunEverything ?? null,
      latestRootBlobId: rootId ?? null,
      client: client ?? null,
      blobCount: input.blobs.size,
      opaqueBlobs,
    },
    nodes: folded,
    promptHistory,
  });
};

/**
 * The list-time shape for a chat dir — meta and workspace without decoding
 * the message list. `workspace` is the checkpoint's field-9 path when the
 * repository already opened the store; meta.json `cwd` wins when present.
 */
export const summarizeStore = (
  input: CursorChatInfo & {
    readonly meta?: CursorStoreMeta;
    readonly metaJson?: CursorMetaJson;
    readonly workspace?: string;
    readonly mtimeMs?: number;
  },
): Session => {
  const createdMs =
    input.metaJson?.createdAtMs ?? input.meta?.createdAt ?? input.mtimeMs ?? Date.now();
  const updatedMs = input.metaJson?.updatedAtMs ?? input.mtimeMs ?? createdMs;
  const named = input.metaJson?.title ?? input.meta?.name;
  return Session.make({
    id: input.id,
    title: named !== undefined && named !== "" ? named : input.id,
    workingDirectory: input.metaJson?.cwd ?? input.workspace ?? input.fallbackCwd ?? "/",
    backendType: "cursor",
    agentMode: input.meta?.mode ?? "default",
    model: input.meta?.lastUsedModel ?? "unknown",
    createdAt: Math.floor(createdMs / 1000),
    lastActivityAt: Math.floor(Math.max(createdMs, updatedMs) / 1000),
    mainChainId: 0,
    metadata: {
      source: "cursor",
      store: "chats",
      workspaceHash: input.workspaceHash ?? null,
      agentId: input.meta?.agentId ?? null,
      latestRootBlobId: input.meta?.latestRootBlobId ?? null,
    },
  });
};

/* ------------------------------------------------------------------ */
/* agent-transcripts/*.jsonl → IR (lossy projection)                    */
/* ------------------------------------------------------------------ */

export interface CursorTranscriptSource {
  /** Chat id — the transcript dir/file name. */
  readonly id: string;
  /** Project dir name; decodes to the working directory when no cwd exists. */
  readonly projectSlug: string;
  /** Set when the file lives under `<chat>/subagents/` — the parent chat id. */
  readonly parentSessionId?: string;
  /** File mtime in ms — the only timestamp the projection has. */
  readonly mtimeMs?: number;
}

const parseLines = (raw: string): ReadonlyArray<Record<string, unknown>> =>
  raw.split("\n").flatMap((line) => {
    const trimmed = line.trim();
    if (trimmed === "") return [];
    try {
      const parsed: unknown = JSON.parse(trimmed);
      return isObject(parsed) ? [parsed] : [];
    } catch {
      return [];
    }
  });

interface TranscriptMeta {
  readonly firstUserText: string | undefined;
  readonly promptHistory: ReadonlyArray<PromptHistoryEntry>;
  readonly turnErrors: ReadonlyArray<unknown>;
}

const transcriptMeta = (
  lines: ReadonlyArray<Record<string, unknown>>,
  tsMs: number,
): TranscriptMeta => {
  let firstUserText: string | undefined;
  const promptHistory: Array<PromptHistoryEntry> = [];
  const turnErrors: Array<unknown> = [];
  for (const line of lines) {
    if (line.type === "turn_ended") {
      if (line.status === "error") turnErrors.push(line.error ?? null);
      continue;
    }
    if (line.role !== "user") continue;
    const message = isObject(line.message) ? line.message : undefined;
    const content = message === undefined ? undefined : message.content;
    const text = Array.isArray(content)
      ? content
          .flatMap((item) =>
            isObject(item) && item.type === "text" && typeof item.text === "string"
              ? [item.text]
              : [],
          )
          .join("\n")
      : typeof content === "string"
        ? content
        : "";
    const query = extractUserQuery(text);
    if (query === undefined) continue;
    firstUserText ??= query;
    promptHistory.push(PromptHistoryEntry.make({ content: sanitize(query), timestamp: tsMs }));
  }
  return { firstUserText, promptHistory, turnErrors };
};

/**
 * `[REDACTED]` inside a text block is the transcript's projection of a
 * redacted reasoning block — strip it and mark thinking.
 */
const stripRedacted = (text: string): { text: string; redacted: boolean } => {
  if (!text.includes("[REDACTED]")) return { text, redacted: false };
  return { text: text.replace(/\s*\[REDACTED\]/g, ""), redacted: true };
};

const transcriptNodes = (
  lines: ReadonlyArray<Record<string, unknown>>,
  ts: number,
): ReadonlyArray<MessageNode> => {
  const nodes: Array<MessageNode> = [];
  let lastNodeId: number | null = null;

  const push = (make: (nodeId: number, parent: Option.Option<number>) => MessageNode): void => {
    const parent = lastNodeId === null ? Option.none<number>() : Option.some(lastNodeId);
    const node = make(nodes.length, parent);
    nodes.push(node);
    lastNodeId = node.nodeId;
  };

  for (const line of lines) {
    const message = isObject(line.message) ? line.message : undefined;
    const content = message === undefined ? undefined : message.content;
    const blocks = Array.isArray(content) ? content : [];

    if (line.role === "user") {
      const textParts = blocks.flatMap((item) =>
        isObject(item) && item.type === "text" && typeof item.text === "string" ? [item.text] : [],
      );
      const text =
        textParts.length > 0 ? textParts.join("\n") : typeof content === "string" ? content : "";
      if (text === "") continue;
      push((nodeId, parent) =>
        MessageNode.make({
          nodeId,
          parentNodeId: parent,
          role: "user",
          content: sanitize(text),
          createdAt: ts,
          metadata: { projection: "transcript" },
        }),
      );
      continue;
    }

    if (line.role === "assistant") {
      const textParts: Array<string> = [];
      const toolCalls: Array<ToolCall> = [];
      let redacted = false;
      for (const item of blocks) {
        if (!isObject(item)) continue;
        if (item.type === "text" && typeof item.text === "string") {
          const cleaned = stripRedacted(item.text);
          if (cleaned.redacted) redacted = true;
          if (cleaned.text !== "") textParts.push(cleaned.text);
          continue;
        }
        if (item.type === "tool_use") {
          const name = strField(item, "name") ?? "unknown";
          toolCalls.push(
            ToolCall.make({
              id: strField(item, "id") ?? `cursor-tool-${nodes.length}-${toolCalls.length}`,
              name,
              arguments: item.input === undefined ? {} : item.input,
              index: toolCalls.length,
            }),
          );
        }
      }
      if (textParts.length === 0 && toolCalls.length === 0 && !redacted) continue;
      push((nodeId, parent) =>
        MessageNode.make({
          nodeId,
          parentNodeId: parent,
          role: "assistant",
          content: sanitize(textParts.join("\n")),
          thinking: redacted ? Option.some(REDACTED_THINKING) : Option.none(),
          toolCalls,
          createdAt: ts,
          metadata: { projection: "transcript", ...(redacted ? { redactedReasoning: true } : {}) },
        }),
      );
      continue;
    }
    // `turn_ended` markers and unknown roles emit no node.
  }
  return nodes;
};

const transcriptSession = (
  lines: ReadonlyArray<Record<string, unknown>>,
  source: CursorTranscriptSource,
  nodes: ReadonlyArray<MessageNode>,
): Session => {
  const tsMs = source.mtimeMs ?? Date.now();
  const meta = transcriptMeta(lines, tsMs);
  return Session.make({
    id: source.id,
    title: meta.firstUserText !== undefined ? meta.firstUserText.slice(0, 80) : source.id,
    workingDirectory: ClaudeCode.decodeProjectDir(source.projectSlug),
    backendType: "cursor",
    agentMode: "default",
    model: "unknown",
    createdAt: Math.floor(tsMs / 1000),
    lastActivityAt: Math.floor(tsMs / 1000),
    mainChainId: nodes.length > 0 ? nodes[nodes.length - 1].nodeId : 0,
    parentSessionId: Option.fromNullable(source.parentSessionId),
    metadata: {
      source: "cursor",
      store: "transcript",
      project: source.projectSlug,
      lossy: true,
      turnErrors: meta.turnErrors,
    },
    nodes,
    promptHistory: meta.promptHistory,
  });
};

/**
 * A transcript projection into full IR: text and `tool_use` blocks map to
 * nodes, but the projection carries no tool results, usage, or per-message
 * timestamps — every node is stamped with the file mtime.
 */
export const fromTranscriptJsonl = (raw: string, source: CursorTranscriptSource): Session => {
  const lines = parseLines(raw);
  const tsMs = source.mtimeMs ?? Date.now();
  return transcriptSession(lines, source, transcriptNodes(lines, Math.floor(tsMs / 1000)));
};

/** The list-time shape: transcript meta without building nodes. */
export const summarizeTranscriptJsonl = (raw: string, source: CursorTranscriptSource): Session =>
  transcriptSession(parseLines(raw), source, []);
