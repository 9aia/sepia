import { createHash } from "node:crypto";
import { resolve as resolvePath } from "node:path";
import { Option } from "effect";
import {
  MessageNode,
  PromptHistoryEntry,
  REDACTED_THINKING,
  Session,
  ToolCall,
  type ToolCallDiff,
  type ToolCallLocation,
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
 * `[redacted]` thinking marker. There is no ACP runtime.
 *
 * Writes target both stores. The canonical `store.db` path is fully
 * synthesised (`storeWritePlan`): blob ids are the sha256 of their bytes,
 * the checkpoint protobuf carries the ordered field-1 message refs plus
 * the field-9 workspace URI, field-10 flag and field-22 `"cli"` tag, and
 * `meta['0']` is the hex-encoded JSON row pointing at it. The checkpoint
 * fields a real Cursor writes for bookkeeping — field-5 token stats and
 * the field-8 groups of prompt/context/step/tool-detail records — are not
 * synthesised: nothing on the resume path reads them, and inventing their
 * semantics risks a malformed DAG. `toTranscriptJsonl` still mirrors the
 * lossy projection alongside, matching Cursor's own dual write.
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

/* ------------------------------------------------------------------ */
/* Tool inputs → locations/diffs (shared by store + transcript)         */
/* ------------------------------------------------------------------ */

/**
 * One `*** <Verb> File:` section of an `ApplyPatch` payload. V4A patches are
 * the only Cursor tool input that is a raw string rather than an object —
 * hunks (`@@` … context/`-`/`+` lines) still map onto the same
 * `{oldText, newText}` diff shape a restore reverse-applies.
 */
const applyPatchRefs = (
  patch: string,
): {
  readonly locations: ReadonlyArray<ToolCallLocation>;
  readonly diffs: ReadonlyArray<ToolCallDiff>;
} => {
  const locations: Array<ToolCallLocation> = [];
  const diffs: Array<ToolCallDiff> = [];
  // Path of the section currently accumulating hunks; `Move to` redirects
  // it — the rename itself isn't a diff, but the edit lands on the new name.
  let path: string | undefined;
  let verb: "add" | "update" | "delete" | undefined;
  let hunk: { oldLines: Array<string>; newLines: Array<string> } | undefined;

  const flushHunk = (): void => {
    // A hunk that recorded no payload lines still leaves a bare `{path}`
    // entry — the change is on record even when nothing is revertable.
    if (hunk !== undefined && path !== undefined) {
      const oldText = hunk.oldLines.join("\n");
      const newText = hunk.newLines.join("\n");
      diffs.push({
        path,
        ...(oldText === "" ? {} : { oldText }),
        ...(newText === "" ? {} : { newText }),
      });
    }
    hunk = undefined;
  };
  const flushSection = (): void => {
    flushHunk();
    verb = undefined;
  };
  const startSection = (nextVerb: typeof verb, nextPath: string): void => {
    flushSection();
    verb = nextVerb;
    path = nextPath;
    locations.push({ path });
    if (nextVerb === "add" || nextVerb === "delete") hunk = { oldLines: [], newLines: [] };
  };

  for (const raw of patch.split("\n")) {
    const line = raw.replace(/\r$/, "");
    // `Add/Update/Delete File`/`Move to` take `: <path>`; `Begin/End Patch`
    // are bare sentinels with no operand.
    const op =
      /^\*\*\*\s*(Add File|Update File|Delete File|Move to|End Patch|Begin Patch)\s*(?::\s*(.*))?$/.exec(
        line,
      );
    if (op !== null) {
      const [, directive, operand = ""] = op;
      switch (directive) {
        case "Add File":
          startSection("add", operand);
          break;
        case "Update File":
          startSection("update", operand);
          break;
        case "Delete File":
          startSection("delete", operand);
          break;
        case "Move to":
          // The file was renamed then edited — hunks land on the new name;
          // the old name keeps only its location entry.
          if (operand !== "") {
            path = operand;
            locations.push({ path });
          }
          break;
        case "End Patch":
          flushSection();
          break;
      }
      continue;
    }
    if (line.startsWith("@@")) {
      // New hunk inside an update section (the marker's trailing context is
      // a locate hint, not part of the recorded change).
      flushHunk();
      if (verb === "update") hunk = { oldLines: [], newLines: [] };
      continue;
    }
    if (hunk === undefined) continue;
    if (line === "\\ No newline at end of file") continue;
    const marker = line[0];
    const body = line.slice(1);
    if (marker === " " || marker === "") {
      // Context lines belong to both sides; a bare line is context too.
      hunk.oldLines.push(line === "" ? "" : body);
      hunk.newLines.push(line === "" ? "" : body);
      continue;
    }
    if (marker === "-") hunk.oldLines.push(body);
    else if (marker === "+") hunk.newLines.push(body);
  }
  flushSection();
  return { locations, diffs };
};

/**
 * File paths a Cursor tool call's args name — `StrReplace`/`Write` inputs
 * carry the before/after payloads a restore reverse-applies (the same
 * `{old,new}_string` contract Cline's `editor` uses), `Delete` records only
 * the path it removed, and `ApplyPatch`'s raw patch string decodes through
 * `applyPatchRefs`. Read-style tools contribute locations.
 */
export const toolFileRefs = (
  name: string,
  args: unknown,
): {
  readonly locations: ReadonlyArray<ToolCallLocation>;
  readonly diffs: ReadonlyArray<ToolCallDiff>;
} => {
  if (name === "ApplyPatch") {
    // The args are the patch itself — a bare string, or an object holding it
    // under a patch-ish key.
    const text =
      typeof args === "string"
        ? args
        : isObject(args)
          ? (strField(args, "patch") ??
            strField(args, "input") ??
            strField(args, "content") ??
            strField(args, "diff"))
          : undefined;
    return text === undefined ? { locations: [], diffs: [] } : applyPatchRefs(text);
  }
  if (!isObject(args)) return { locations: [], diffs: [] };
  const path = strField(args, "path") ?? strField(args, "target_notebook");
  const locations: Array<ToolCallLocation> = path === undefined ? [] : [{ path }];
  for (const key of ["paths", "target_directories"] as const) {
    const list = args[key];
    if (Array.isArray(list)) {
      for (const item of list) {
        if (typeof item === "string" && item !== "") locations.push({ path: item });
      }
    }
  }
  const dir = strField(args, "target_directory");
  if (dir !== undefined) locations.push({ path: dir });

  switch (name) {
    case "StrReplace":
    case "Edit":
    case "EditNotebook": {
      if (path === undefined) return { locations, diffs: [] };
      const oldText = strField(args, "old_string");
      const newText = strField(args, "new_string");
      return {
        locations,
        diffs:
          oldText === undefined && newText === undefined
            ? []
            : [
                {
                  path,
                  ...(oldText === undefined ? {} : { oldText }),
                  ...(newText === undefined ? {} : { newText }),
                },
              ],
      };
    }
    case "Write": {
      const content = strField(args, "contents") ?? strField(args, "content");
      return {
        locations,
        diffs: path === undefined || content === undefined ? [] : [{ path, newText: content }],
      };
    }
    default:
      return { locations, diffs: [] };
  }
};

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
        const refs = toolFileRefs(name, args);
        toolCalls.push(
          ToolCall.make({
            id,
            name,
            arguments: args,
            index: toolCalls.length,
            locations: refs.locations,
            diffs: refs.diffs,
          }),
        );
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
          const args = item.input === undefined ? {} : item.input;
          const refs = toolFileRefs(name, args);
          toolCalls.push(
            ToolCall.make({
              id: strField(item, "id") ?? `cursor-tool-${nodes.length}-${toolCalls.length}`,
              name,
              arguments: args,
              index: toolCalls.length,
              locations: refs.locations,
              diffs: refs.diffs,
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

/* ------------------------------------------------------------------ */
/* IR → agent-transcripts projection (write side)                      */
/* ------------------------------------------------------------------ */

/**
 * The project dir name Cursor derives from a working directory — the
 * inverse of `ClaudeCode.decodeProjectDir`: non-alphanumerics flatten to
 * `-` and the leading separator drops (`/home/me/proj` → `home-me-proj`).
 * Lossy the same way the decode is (`my proj` and `my-proj` collide).
 */
export const projectSlugFromCwd = (cwd: string): string => {
  const slug = cwd.replace(/[^a-zA-Z0-9]/g, "-").replace(/^-+|-+$/g, "");
  return slug === "" ? "root" : slug;
};

/**
 * One IR node → the transcript line it projects to; `undefined` when the
 * format has no slot for the role (`system`, `tool` results — Cursor's own
 * writer drops them too).
 */
const transcriptLine = (node: MessageNode): Record<string, unknown> | undefined => {
  if (node.role === "user") {
    if (node.content === "") return undefined;
    return {
      role: "user",
      message: { content: [{ type: "text", text: node.content }] },
    };
  }
  if (node.role === "assistant") {
    // Sealed/recorded thinking projects the way Cursor's own writer renders
    // it: a `[REDACTED]` suffix inside the text block.
    const text = Option.isSome(node.thinking)
      ? node.content === ""
        ? "[REDACTED]"
        : `${node.content}\n\n[REDACTED]`
      : node.content;
    const content: Array<Record<string, unknown>> = [];
    if (text !== "") content.push({ type: "text", text });
    for (const call of node.toolCalls) {
      content.push({ type: "tool_use", id: call.id, name: call.name, input: call.arguments });
    }
    if (content.length === 0) return undefined;
    return { role: "assistant", message: { content } };
  }
  return undefined;
};

/**
 * Encode a session into the transcript projection's JSONL. Real
 * transcripts carry `{role, message:{content}}` lines only, so user text,
 * assistant text and `tool_use` blocks survive while tool results, system
 * prompts, per-message timestamps, usage and the title are dropped — the
 * same loss Cursor's own projection accepts. `id` rides on `tool_use`
 * even though Cursor omits it: the reader honours it and a converted
 * session keeps its call ids.
 */
export const toTranscriptJsonl = (session: Session): string =>
  session.nodes
    .flatMap((node) => {
      const line = transcriptLine(node);
      return line === undefined ? [] : [JSON.stringify(line)];
    })
    .join("\n") + "\n";

/* ------------------------------------------------------------------ */
/* IR → store.db (canonical write side)                               */
/* ------------------------------------------------------------------ */

const encode = (text: string): Uint8Array => new TextEncoder().encode(text);

const hashHex = (algorithm: string, data: Uint8Array | string): string =>
  createHash(algorithm).update(data).digest("hex");

/**
 * A blob's `blobs.id` is the lowercase hex sha256 of its bytes — verified
 * against real stores, where `e3b0c44…` (sha256 of nothing) is the empty
 * blob every store keeps and the root of a chat with no messages yet.
 */
export const blobIdFor = (data: Uint8Array): string => hashHex("sha256", data);

/**
 * The `chats/<hash>` directory is md5 of the workspace path — the agent
 * hashes `path.resolve(cwd)`, so a trailing slash or `.`/`..` segment is
 * normalised first, same as the real mapping.
 */
export const workspaceHashFromCwd = (cwd: string): string => hashHex("md5", resolvePath(cwd));

/** Inverse of `workspaceFromUri`: each path segment is URI-encoded. */
export const workspaceUriFromCwd = (cwd: string): string =>
  `file://${cwd
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/")}`;

const writeVarint = (value: number): Uint8Array => {
  const out: Array<number> = [];
  let n = value;
  do {
    let b = n & 0x7f;
    n = Math.floor(n / 128);
    if (n > 0) b |= 0x80;
    out.push(b);
  } while (n > 0);
  return new Uint8Array(out);
};

const concatBytes = (parts: ReadonlyArray<Uint8Array>): Uint8Array => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};

const lenField = (fieldNum: number, data: Uint8Array): Uint8Array =>
  concatBytes([writeVarint(fieldNum * 8 + 2), writeVarint(data.length), data]);

const intField = (fieldNum: number, value: number): Uint8Array =>
  concatBytes([writeVarint(fieldNum * 8), writeVarint(value)]);

export interface CheckpointWriteInput {
  /** Ordered sha256 blob ids of the message list — field-1 entries. */
  readonly messageIds: ReadonlyArray<string>;
  /** `file://` workspace URI — from `workspaceUriFromCwd`. */
  readonly workspace?: string;
  /** Client tag written at field 22; real CLI stores use `"cli"`. */
  readonly client?: string;
}

/**
 * Synthesise a checkpoint blob. Only the load-bearing fields are written:
 * the ordered field-1 message refs, the field-9 workspace URI, the
 * field-10 flag and the field-22 client tag. A real checkpoint also
 * carries field-5 token stats and field-8 refs to ancillary record groups
 * (prompt/context/step/tool-detail blobs) — UI bookkeeping the resume
 * path never reads, deliberately omitted rather than guessed at.
 * `decodeCheckpoint` round-trips everything written here.
 */
export const encodeCheckpoint = (input: CheckpointWriteInput): Uint8Array => {
  const parts: Array<Uint8Array> = [];
  for (const id of input.messageIds) {
    const bytes = fromHex(id);
    if (bytes !== undefined && bytes.length === 32) parts.push(lenField(1, bytes));
  }
  if (input.workspace !== undefined) parts.push(lenField(9, encode(input.workspace)));
  parts.push(intField(10, 1));
  parts.push(lenField(22, encode(input.client ?? "cli")));
  return concatBytes(parts);
};

export interface StoreMetaWriteInput {
  readonly agentId: string;
  readonly latestRootBlobId: string;
  readonly name?: string;
  readonly mode?: string;
  readonly isRunEverything?: boolean;
  /** Epoch milliseconds. */
  readonly createdAt?: number;
  readonly lastUsedModel?: string;
}

/** The `meta['0']` row — hex of the UTF-8 JSON, matching `parseStoreMeta`. */
export const encodeStoreMeta = (meta: StoreMetaWriteInput): string =>
  toHex(
    encode(
      JSON.stringify({
        agentId: meta.agentId,
        latestRootBlobId: meta.latestRootBlobId,
        ...(meta.name === undefined ? {} : { name: meta.name }),
        ...(meta.mode === undefined ? {} : { mode: meta.mode }),
        ...(meta.isRunEverything === undefined ? {} : { isRunEverything: meta.isRunEverything }),
        ...(meta.createdAt === undefined ? {} : { createdAt: meta.createdAt }),
        ...(meta.lastUsedModel === undefined ? {} : { lastUsedModel: meta.lastUsedModel }),
      }),
    ),
  );

export interface MetaJsonWriteInput {
  readonly createdAtMs: number;
  readonly updatedAtMs: number;
  readonly title?: string;
  readonly hasConversation: boolean;
  readonly cwd?: string;
}

/** The `meta.json` sidecar — schemaVersion 1, key order matching real files. */
export const encodeMetaJson = (meta: MetaJsonWriteInput): string =>
  JSON.stringify({
    schemaVersion: 1,
    createdAtMs: meta.createdAtMs,
    hasConversation: meta.hasConversation,
    ...(meta.title === undefined || meta.title === "" ? {} : { title: meta.title }),
    updatedAtMs: meta.updatedAtMs,
    ...(meta.cwd === undefined ? {} : { cwd: meta.cwd }),
  });

const nodeContext = (node: MessageNode): string | undefined =>
  isObject(node.metadata) && typeof node.metadata.context === "string"
    ? node.metadata.context
    : undefined;

const nodeBlobId = (node: MessageNode): string | undefined =>
  isObject(node.metadata) && typeof node.metadata.blobId === "string"
    ? node.metadata.blobId
    : undefined;

/**
 * A deterministic `providerOptions.cursor.requestId` — real stores mint a
 * random uuid per user turn, but deriving one from session+node keeps a
 * repeated `save` byte-identical: a random id would orphan a fresh message
 * and checkpoint blob on every write.
 */
const derivedRequestId = (sessionId: string, nodeId: number): string => {
  const hex = hashHex("sha256", `cursor-request:${sessionId}:${nodeId}`);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-a${hex.slice(17, 20)}-${hex.slice(20, 32)}`;
};

const toolResultJson = (node: MessageNode): Record<string, unknown> => ({
  type: "tool-result",
  ...(Option.isSome(node.toolCallId) ? { toolCallId: node.toolCallId.value } : {}),
  ...(Option.isSome(node.toolName) ? { toolName: node.toolName.value } : {}),
  result: node.content,
});

/** `highLevelToolCallResult.output` rebuilt from the IR's status + duration. */
const toolResultOutput = (node: MessageNode): Record<string, unknown> => {
  const info = Option.getOrUndefined(node.toolResult);
  return {
    isError: info?.status === "error",
    ...(info?.durationMs === undefined ? {} : { success: { executionTime: info.durationMs } }),
  };
};

/**
 * One IR node → the AI-SDK JSON it serialises to; `undefined` when the
 * node carries nothing a message blob can hold (empty text, an assistant
 * turn with no content, calls or thinking). Shapes mirror what the reader
 * decodes: `user` context nodes keep the raw string `content` form while
 * queries use `[{type:"text"}]` + a requestId; `assistant` packs
 * `redacted-reasoning`/`text`/`tool-call` items and records `id: "1"` like
 * every observed assistant blob; `tool` results lift the call id to the
 * top-level `id` and re-encode `toolResult` under
 * `providerOptions.cursor.highLevelToolCallResult`.
 */
const messageJson = (sessionId: string, node: MessageNode): Record<string, unknown> | undefined => {
  if (node.role === "system") {
    return node.content === "" ? undefined : { role: "system", content: node.content };
  }
  if (node.role === "user") {
    if (node.content === "") return undefined;
    // `<user_info>` plumbing is a raw string in real stores, not a parts list.
    if (nodeContext(node) === "user_info") {
      return { role: "user", content: node.content };
    }
    return {
      role: "user",
      content: [{ type: "text", text: node.content }],
      providerOptions: {
        cursor: {
          requestId:
            Option.getOrUndefined(node.requestId) ?? derivedRequestId(sessionId, node.nodeId),
        },
      },
    };
  }
  if (node.role === "assistant") {
    const content: Array<Record<string, unknown>> = [];
    if (Option.isSome(node.thinking)) {
      content.push({
        type: "redacted-reasoning",
        ...(Option.isSome(node.thinkingSignature) ? { data: node.thinkingSignature.value } : {}),
      });
    }
    if (node.content !== "") content.push({ type: "text", text: node.content });
    for (const call of node.toolCalls) {
      content.push({
        type: "tool-call",
        toolCallId: call.id,
        toolName: call.name,
        args: call.arguments ?? {},
      });
    }
    if (content.length === 0) return undefined;
    return { role: "assistant", id: "1", content };
  }
  if (node.role === "tool") {
    return {
      role: "tool",
      ...(Option.isSome(node.toolCallId) ? { id: node.toolCallId.value } : {}),
      content: [toolResultJson(node)],
      providerOptions: {
        cursor: { highLevelToolCallResult: { output: toolResultOutput(node) } },
      },
    };
  }
  // `role` is a closed union — the guards above cover every case, so
  // falling through here (undefined) is unreachable by construction.
};

export interface StoreBlobWrite {
  /** `blobs.id` — the sha256 hex of `data`. */
  readonly id: string;
  readonly data: Uint8Array;
}

/**
 * The ordered message-blob plan: entry `i` is checkpoint field-1 ref `i`.
 * Consecutive `tool` nodes that a real store grouped under one blob id
 * (recorded on `metadata.blobId`) re-group into a single blob; every other
 * node is one blob each. `result` strings are the node's content verbatim.
 */
export const messageBlobsFromSession = (session: Session): ReadonlyArray<StoreBlobWrite> => {
  const blobs: Array<StoreBlobWrite> = [];
  const push = (json: Record<string, unknown>): void => {
    const data = encode(JSON.stringify(json));
    blobs.push({ id: blobIdFor(data), data });
  };

  let group:
    | { blobId: string; results: Array<Record<string, unknown>>; node: MessageNode }
    | undefined;
  const flushGroup = (): void => {
    if (group === undefined) return;
    push({
      role: "tool",
      ...(Option.isSome(group.node.toolCallId) ? { id: group.node.toolCallId.value } : {}),
      content: group.results,
      providerOptions: {
        cursor: { highLevelToolCallResult: { output: toolResultOutput(group.node) } },
      },
    });
    group = undefined;
  };

  for (const node of session.nodes) {
    if (node.role !== "tool") {
      flushGroup();
      const json = messageJson(session.id, node);
      if (json !== undefined) push(json);
      continue;
    }
    const shared = nodeBlobId(node);
    if (shared !== undefined && group?.blobId === shared) {
      group.results.push(toolResultJson(node));
      continue;
    }
    flushGroup();
    if (shared !== undefined) {
      group = { blobId: shared, results: [toolResultJson(node)], node };
    } else {
      const json = messageJson(session.id, node);
      if (json !== undefined) push(json);
    }
  }
  flushGroup();
  return blobs;
};

export interface StoreWritePlan {
  /** Every row `save` inserts into `blobs` — message blobs then the root. */
  readonly blobs: ReadonlyArray<StoreBlobWrite>;
  /** sha256 of the checkpoint — `meta['0'].latestRootBlobId`. */
  readonly rootBlobId: string;
  /** Hex-encoded `meta['0']` JSON row. */
  readonly metaRow: string;
  /** `meta.json` sidecar content. */
  readonly metaJson: string;
  /** `prompt_history.json` content — undefined when nothing is provable. */
  readonly promptHistoryJson?: string;
}

/**
 * Everything a `store.db` + sidecar write needs for one session. An empty
 * session gets the empty blob as its root — exactly what a fresh real
 * chat's store records (`latestRootBlobId` = sha256 of nothing).
 * `createdAtMs` lets a rewrite keep the chat's original creation time.
 */
export const storeWritePlan = (
  session: Session,
  options: { readonly createdAtMs?: number } = {},
): StoreWritePlan => {
  const emptyData = new Uint8Array(0);
  const messages = messageBlobsFromSession(session);
  const checkpoint =
    messages.length === 0
      ? emptyData
      : encodeCheckpoint({
          messageIds: messages.map((blob) => blob.id),
          workspace: workspaceUriFromCwd(session.workingDirectory),
          client: "cli",
        });
  const rootBlobId = blobIdFor(checkpoint);
  const metadata = isObject(session.metadata) ? session.metadata : {};
  const createdAtMs = options.createdAtMs ?? session.createdAt * 1000;

  const prompts =
    session.promptHistory.length > 0
      ? session.promptHistory.map((entry) => entry.content)
      : session.nodes.flatMap((node) => {
          if (node.role !== "user" || nodeContext(node) === "user_info") return [];
          const query = extractUserQuery(node.content);
          return query === undefined ? [] : [query];
        });

  return {
    blobs: [
      { id: blobIdFor(emptyData), data: emptyData },
      ...messages,
      { id: rootBlobId, data: checkpoint },
    ],
    rootBlobId,
    metaRow: encodeStoreMeta({
      agentId: session.id,
      latestRootBlobId: rootBlobId,
      name: session.title === "" ? undefined : session.title,
      mode:
        typeof metadata.mode === "string"
          ? metadata.mode
          : session.agentMode === ""
            ? undefined
            : session.agentMode,
      isRunEverything:
        typeof metadata.isRunEverything === "boolean" ? metadata.isRunEverything : undefined,
      createdAt: createdAtMs,
      lastUsedModel:
        session.model === "" || session.model === "unknown" ? "default" : session.model,
    }),
    metaJson: encodeMetaJson({
      createdAtMs,
      updatedAtMs: Math.max(session.lastActivityAt * 1000, createdAtMs),
      title: session.title,
      hasConversation: session.nodes.length > 0,
      cwd: session.workingDirectory,
    }),
    promptHistoryJson: prompts.length === 0 ? undefined : JSON.stringify(prompts),
  };
};
