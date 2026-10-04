import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Effect, Option } from "effect";
import {
  ConversionError,
  MessageNode,
  PromptHistoryEntry,
  Session,
  ToolCall,
  type Block,
  type TokenUsage,
  type ToolResultInfo,
} from "./Domain.js";
import * as Devin from "./Devin.js";

/**
 * Claude Code keeps one append-only JSONL transcript per session at
 * `~/.claude/projects/<slug>/<session-uuid>.jsonl`, where `<slug>` is the
 * working directory with non-alphanumeric characters replaced by `-`.
 * Sub-agent (Task tool) transcripts live in `<uuid>/subagents/agent-*.jsonl`
 * (current layout) or as `agent-*.jsonl` siblings (legacy layout); every
 * entry in them carries `isSidechain: true` and the parent's `sessionId`.
 *
 * This module reads that format into the session IR. There is no writer:
 * Claude Code resumes sessions from its own files, and sepia treats the
 * store as read-only.
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

/** `timestamp` is ISO-8601 with millis; the IR keeps epoch seconds. */
const toSeconds = (value: unknown): number | undefined =>
  typeof value === "string" && !Number.isNaN(new Date(value).getTime())
    ? Math.floor(new Date(value).getTime() / 1000)
    : undefined;

/**
 * The project dir name is the cwd with non-alphanumerics flattened to `-`,
 * so decoding is lossy — `-home-me-proj` → `/home/me/proj` recovers the
 * common case. Used only when no entry in the file carries a `cwd`.
 */
export const decodeProjectDir = (name: string): string => {
  const decoded = name.replaceAll("-", "/");
  return decoded.startsWith("/") ? decoded : `/${decoded}`;
};

/**
 * A `message.content` array item mapped onto the IR block union.
 * `tool_use`/`tool_result`/`thinking` have dedicated IR fields and are
 * skipped here; `image`/`document` are the attachment forms (`source`
 * carries `base64`/`url`/`text` variants).
 */
const blockFromClaude = (item: unknown): Block | undefined => {
  if (!isObject(item)) return undefined;
  const source = isObject(item.source) ? item.source : undefined;
  const sourceField = (key: string): string | undefined =>
    source === undefined ? undefined : strField(source, key);
  switch (item.type) {
    case "text": {
      const text = strField(item, "text");
      return text === undefined ? undefined : { type: "text", text };
    }
    case "image": {
      const data = strField(item, "data") ?? sourceField("data");
      const uri = strField(item, "url") ?? sourceField("url");
      if (data === undefined && uri === undefined) return undefined;
      const mimeType =
        sourceField("media_type") ?? strField(item, "media_type") ?? strField(item, "mimeType");
      return {
        type: "image",
        ...(data === undefined ? {} : { data }),
        ...(uri === undefined ? {} : { uri }),
        ...(mimeType === undefined ? {} : { mimeType }),
      };
    }
    case "document": {
      const text = sourceField("text") ?? strField(item, "text");
      const data = sourceField("data") ?? strField(item, "data");
      const uri = sourceField("url") ?? strField(item, "url");
      if (text === undefined && data === undefined && uri === undefined) return undefined;
      const mimeType =
        sourceField("media_type") ?? strField(item, "media_type") ?? strField(item, "mimeType");
      return {
        type: "file",
        ...(uri === undefined ? {} : { uri }),
        ...(strField(item, "title") === undefined ? {} : { name: strField(item, "title") }),
        ...(mimeType === undefined ? {} : { mimeType }),
        ...(text === undefined ? {} : { text }),
        ...(data === undefined ? {} : { data }),
      };
    }
    default:
      return undefined;
  }
};

/**
 * Claude's per-message `usage` — `input_tokens`/`output_tokens` plus cache
 * tiers (`cache_read_input_tokens`, `cache_creation_input_tokens` or the
 * nested `cache_creation.ephemeral_*` split) — onto IR `TokenUsage`.
 */
export const usageFromClaude = (usage: unknown): Option.Option<TokenUsage> => {
  if (!isObject(usage)) return Option.none();
  const input = finiteNumber(usage.input_tokens);
  const output = finiteNumber(usage.output_tokens);
  if (input === undefined && output === undefined) return Option.none();
  const cacheRead = finiteNumber(usage.cache_read_input_tokens);
  const creation = isObject(usage.cache_creation) ? usage.cache_creation : undefined;
  const cacheWrite =
    finiteNumber(usage.cache_creation_input_tokens) ??
    (creation === undefined
      ? undefined
      : (finiteNumber(creation.ephemeral_5m_input_tokens) ?? 0) +
        (finiteNumber(creation.ephemeral_1h_input_tokens) ?? 0));
  return Option.some({
    input: input ?? 0,
    output: output ?? 0,
    ...(cacheRead === undefined ? {} : { cacheRead }),
    ...(cacheWrite === undefined ? {} : { cacheWrite }),
  });
};

/** A `tool_result` block's `content` is a string or a block array. */
const toolResultText = (content: unknown): string => {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    const parts = content.flatMap((item) =>
      isObject(item) && item.type === "text" && typeof item.text === "string" ? [item.text] : [],
    );
    return parts.length > 0 ? parts.join("\n") : JSON.stringify(content);
  }
  return content === undefined || content === null ? "" : JSON.stringify(content);
};

/** The text a `user` entry carries, or undefined for tool-result-only/meta-less entries. */
const userEntryText = (entry: Record<string, unknown>): string | undefined => {
  const content = isObject(entry.message) ? entry.message.content : undefined;
  if (typeof content === "string") return content === "" ? undefined : content;
  if (Array.isArray(content)) {
    const text = content
      .flatMap((item) =>
        isObject(item) && item.type === "text" && typeof item.text === "string" ? [item.text] : [],
      )
      .join("\n");
    return text === "" ? undefined : text;
  }
  return undefined;
};

/** What a file's lines prove about the session, without building nodes. */
interface SessionMeta {
  readonly title: string | undefined;
  readonly cwd: string | undefined;
  readonly model: string | undefined;
  readonly gitBranch: string | undefined;
  readonly claudeVersion: string | undefined;
  readonly slug: string | undefined;
  /** `sessionId` the entries name — the parent's id inside a subagent file. */
  readonly sessionId: string | undefined;
  /** `agentId` the entries name — set on every line of a subagent file. */
  readonly agentId: string | undefined;
  readonly permissionMode: string | undefined;
  readonly createdAt: number | undefined;
  readonly lastActivityAt: number | undefined;
  readonly sawSidechain: boolean;
  readonly firstUserText: string | undefined;
  readonly promptHistory: ReadonlyArray<PromptHistoryEntry>;
}

const collectMeta = (entries: ReadonlyArray<Record<string, unknown>>): SessionMeta => {
  let title: string | undefined;
  let cwd: string | undefined;
  let model: string | undefined;
  let gitBranch: string | undefined;
  let claudeVersion: string | undefined;
  let slug: string | undefined;
  let sessionId: string | undefined;
  let agentId: string | undefined;
  let permissionMode: string | undefined;
  let createdAt: number | undefined;
  let lastActivityAt: number | undefined;
  let sawSidechain = false;
  let firstUserText: string | undefined;
  const promptHistory: Array<PromptHistoryEntry> = [];

  for (const entry of entries) {
    const ts = toSeconds(entry.timestamp);
    if (ts !== undefined) {
      createdAt ??= ts;
      lastActivityAt = ts;
    }
    cwd ??= strField(entry, "cwd");
    sessionId ??= strField(entry, "sessionId");
    agentId ??= strField(entry, "agentId");
    permissionMode ??= strField(entry, "permissionMode");
    claudeVersion ??= strField(entry, "version");
    slug ??= strField(entry, "slug");
    const branch = strField(entry, "gitBranch");
    if (branch !== undefined) gitBranch = branch;
    if (entry.isSidechain === true) sawSidechain = true;

    if (entry.type === "summary") {
      title ??= strField(entry, "summary");
      continue;
    }
    if (entry.type === "assistant") {
      const message = isObject(entry.message) ? entry.message : undefined;
      const m = message === undefined ? undefined : strField(message, "model");
      if (m !== undefined) model = m;
      continue;
    }
    if (entry.type === "user") {
      const text = userEntryText(entry);
      if (text === undefined) continue;
      firstUserText ??= text;
      // `isMeta` lines are command plumbing (`/clear`, local-command output),
      // not prompts the user typed.
      if (entry.isMeta === true) continue;
      promptHistory.push(
        PromptHistoryEntry.make({
          content: sanitize(text),
          timestamp: (ts ?? createdAt ?? 0) * 1000,
          isShell: false,
        }),
      );
    }
  }

  return {
    title,
    cwd,
    model,
    gitBranch,
    claudeVersion,
    slug,
    sessionId,
    agentId,
    permissionMode,
    createdAt,
    lastActivityAt,
    sawSidechain,
    firstUserText,
    promptHistory,
  };
};

/** Provenance flags worth keeping on the node — everything else the entry carried is noise. */
const nodeMeta = (entry: Record<string, unknown>): Record<string, unknown> => ({
  uuid: strField(entry, "uuid") ?? null,
  ...(entry.isSidechain === true ? { isSidechain: true } : {}),
  ...(entry.isMeta === true ? { isMeta: true } : {}),
  ...(entry.isCompactSummary === true ? { isCompactSummary: true } : {}),
  ...(entry.isVisibleInTranscriptOnly === true ? { isVisibleInTranscriptOnly: true } : {}),
});

/**
 * The `parentUuid`/`uuid` chain onto `parentNodeId`/`nodeId`. Every entry
 * records its own parent link — even types that emit no node
 * (`file-history-snapshot`, `queue-operation`, `progress`) — so resolving a
 * parent walks up through skipped entries to the nearest emitted node. An
 * explicit `null` starts a new root (sidechain roots inside a mixed legacy
 * file); a dangling id falls back to the previous node, which is what the
 * append order already gives a linear thread.
 */
const buildNodes = (
  entries: ReadonlyArray<Record<string, unknown>>,
  defaultTs: number,
): ReadonlyArray<MessageNode> => {
  const nodes: Array<MessageNode> = [];
  const nodeIdByUuid = new Map<string, number>();
  const parentByUuid = new Map<string, unknown>();
  const toolNameById = new Map<string, string>();
  const toolArgsById = new Map<string, unknown>();
  let lastNodeId: number | null = null;

  const resolveParent = (parentUuid: unknown): Option.Option<number> => {
    if (parentUuid === null) return Option.none();
    if (typeof parentUuid === "string") {
      let cursor: unknown = parentUuid;
      const seen = new Set<string>();
      while (typeof cursor === "string" && !seen.has(cursor)) {
        const nodeId = nodeIdByUuid.get(cursor);
        if (nodeId !== undefined) return Option.some(nodeId);
        seen.add(cursor);
        cursor = parentByUuid.get(cursor);
      }
    }
    // Absent or dangling: keep the append-order chain going.
    return lastNodeId === null ? Option.none() : Option.some(lastNodeId);
  };

  const push = (
    entry: Record<string, unknown>,
    make: (id: number, parent: Option.Option<number>) => MessageNode,
  ): void => {
    const node = make(nodes.length, resolveParent(entry.parentUuid));
    nodes.push(node);
    lastNodeId = node.nodeId;
    if (typeof entry.uuid === "string") nodeIdByUuid.set(entry.uuid, node.nodeId);
  };

  for (const entry of entries) {
    if (typeof entry.uuid === "string") parentByUuid.set(entry.uuid, entry.parentUuid);
    const ts = toSeconds(entry.timestamp) ?? defaultTs;

    if (entry.type === "user") {
      const message = isObject(entry.message) ? entry.message : undefined;
      const content = message === undefined ? undefined : message.content;
      if (typeof content === "string") {
        // Empty `content` strings are plumbing entries (meta commands,
        // continuation markers), not messages.
        if (content === "") continue;
        push(entry, (nodeId, parent) =>
          MessageNode.make({
            nodeId,
            parentNodeId: parent,
            role: "user",
            content: sanitize(content),
            createdAt: ts,
            metadata: nodeMeta(entry),
          }),
        );
        continue;
      }
      if (!Array.isArray(content)) continue;

      const textParts: Array<string> = [];
      const blocks: Array<Block> = [];
      const results: Array<Record<string, unknown>> = [];
      for (const item of content) {
        if (isObject(item) && item.type === "tool_result") {
          results.push(item);
          continue;
        }
        const block = blockFromClaude(item);
        if (block === undefined) continue;
        blocks.push(block);
        if (block.type === "text") textParts.push(block.text);
      }
      if (blocks.length > 0) {
        push(entry, (nodeId, parent) =>
          MessageNode.make({
            nodeId,
            parentNodeId: parent,
            role: "user",
            content: sanitize(textParts.join("\n")),
            blocks: blocks.some((block) => block.type !== "text") ? blocks : [],
            createdAt: ts,
            metadata: nodeMeta(entry),
          }),
        );
      }
      for (const result of results) {
        const callId = strField(result, "tool_use_id");
        const status: ToolResultInfo["status"] = result.is_error === true ? "error" : "success";
        push(entry, (nodeId, parent) =>
          MessageNode.make({
            nodeId,
            parentNodeId: parent,
            role: "tool",
            content: sanitize(toolResultText(result.content)),
            toolCallId: callId === undefined ? Option.none() : Option.some(callId),
            toolName:
              callId === undefined ? Option.none() : Option.fromNullable(toolNameById.get(callId)),
            toolResult: Option.some({ status }),
            createdAt: ts,
            metadata: {
              ...nodeMeta(entry),
              toolArguments: callId === undefined ? null : (toolArgsById.get(callId) ?? null),
              toolUseResult: entry.toolUseResult ?? null,
            },
          }),
        );
      }
      continue;
    }

    if (entry.type === "assistant") {
      const message = isObject(entry.message) ? entry.message : {};
      const content = Array.isArray(message.content) ? message.content : [];
      const textParts: Array<string> = [];
      const thinkingParts: Array<string> = [];
      const toolCalls: Array<ToolCall> = [];
      for (const item of content) {
        if (!isObject(item)) continue;
        if (item.type === "text" && typeof item.text === "string") {
          textParts.push(item.text);
          continue;
        }
        if (item.type === "thinking" && typeof item.thinking === "string") {
          thinkingParts.push(item.thinking);
          continue;
        }
        // `redacted_thinking` is an opaque blob — there is no text to keep.
        if (item.type === "tool_use") {
          const id = strField(item, "id") ?? `claude-tool-${nodes.length}-${toolCalls.length}`;
          const name = strField(item, "name") ?? "unknown";
          const args = item.input === undefined ? {} : item.input;
          toolCalls.push(
            ToolCall.make({
              id,
              name,
              arguments: args,
              index: toolCalls.length,
              kind: "function",
            }),
          );
          toolNameById.set(id, name);
          toolArgsById.set(id, args);
        }
      }
      const thinking = thinkingParts.join("\n");
      push(entry, (nodeId, parent) =>
        MessageNode.make({
          nodeId,
          parentNodeId: parent,
          role: "assistant",
          content: sanitize(textParts.join("\n")),
          thinking: thinking === "" ? Option.none() : Option.some(sanitize(thinking)),
          toolCalls,
          usage: usageFromClaude(message.usage),
          model: Option.fromNullable(strField(message, "model")),
          requestId: Option.fromNullable(strField(entry, "requestId")),
          finishReason: Option.fromNullable(strField(message, "stop_reason")),
          createdAt: ts,
          metadata: {
            ...nodeMeta(entry),
            messageId: strField(message, "id") ?? null,
          },
        }),
      );
      continue;
    }

    if (entry.type === "system") {
      const subtype = strField(entry, "subtype");
      push(entry, (nodeId, parent) =>
        MessageNode.make({
          nodeId,
          parentNodeId: parent,
          role: "system",
          content: sanitize(strField(entry, "content") ?? `[claude ${subtype ?? "system"}]`),
          createdAt: ts,
          metadata: {
            ...nodeMeta(entry),
            subtype: subtype ?? null,
            level: strField(entry, "level") ?? null,
            compactMetadata: entry.compactMetadata ?? null,
          },
        }),
      );
    }
    // `summary`, `file-history-snapshot`, `queue-operation` and unknown types
    // emit no node — their uuid/parentUuid are still recorded above so a
    // later entry's parent link resolves through them.
  }

  return nodes;
};

/** What the file's location says — the repo passes the parts it knows. */
export interface ClaudeSourceInfo {
  /** Session id — the JSONL file's basename without the `.jsonl` extension. */
  readonly id: string;
  /** Working directory to use when no entry records `cwd` (a decoded slug). */
  readonly fallbackCwd?: string;
  /** Parent session id when the layout proves it (`<uuid>/subagents/` files). */
  readonly parentSessionId?: string;
}

const sessionFrom = (
  meta: SessionMeta,
  source: ClaudeSourceInfo,
  nodes: ReadonlyArray<MessageNode>,
): Session => {
  const now = Math.floor(Date.now() / 1000);
  const createdAt = meta.createdAt ?? now;
  const title =
    meta.title ?? meta.firstUserText?.replace(/\s+/g, " ").trim().slice(0, 80) ?? source.id;
  // A subagent file's entries name the parent session (`sessionId`) and mark
  // themselves `isSidechain`; a main file's sidechain-free entries name the
  // file itself, so a mismatch is only trusted once a sidechain was seen.
  const sidechainParent =
    meta.sawSidechain && meta.sessionId !== undefined && meta.sessionId !== source.id
      ? meta.sessionId
      : undefined;
  const agentId =
    meta.agentId ?? (source.id.startsWith("agent-") ? source.id.slice("agent-".length) : undefined);
  return Session.make({
    id: source.id,
    title,
    workingDirectory: meta.cwd ?? source.fallbackCwd ?? "/",
    backendType: "claude",
    agentMode: meta.permissionMode ?? "accept-edits",
    model: meta.model ?? "unknown",
    createdAt,
    lastActivityAt: meta.lastActivityAt ?? createdAt,
    mainChainId: nodes.length > 0 ? nodes[nodes.length - 1].nodeId : 0,
    parentSessionId: Option.fromNullable(sidechainParent ?? source.parentSessionId),
    agentId: Option.fromNullable(agentId),
    metadata: {
      source: "claude-code",
      gitBranch: meta.gitBranch ?? null,
      claudeVersion: meta.claudeVersion ?? null,
      slug: meta.slug ?? null,
    },
    nodes,
    promptHistory: meta.promptHistory,
  });
};

const parseEntries = (raw: string): ReadonlyArray<Record<string, unknown>> =>
  raw.split("\n").flatMap((line) => {
    const trimmed = line.trim();
    if (trimmed === "") return [];
    try {
      const parsed: unknown = JSON.parse(trimmed);
      return isObject(parsed) ? [parsed] : [];
    } catch {
      // Truncated last line of a file being appended mid-write — skip it.
      return [];
    }
  });

/**
 * Parse one Claude Code JSONL transcript into a full session IR: nodes,
 * tree links, usage, tool calls/results and prompt history included.
 * Never throws — blank and malformed lines are skipped.
 */
export const fromJsonl = (raw: string, source: ClaudeSourceInfo): Session => {
  const entries = parseEntries(raw);
  const meta = collectMeta(entries);
  const built = buildNodes(entries, meta.createdAt ?? Math.floor(Date.now() / 1000));
  // Result entries carry `is_error` per call — fold it back onto the issuing
  // `ToolCall`s now that all results have been seen.
  const nodes = Devin.applyToolCallOutcomes(built, Devin.toolNodeOutcomes(built));
  return sessionFrom(meta, source, nodes);
};

/**
 * The list-time shape: same session meta `fromJsonl` computes, without
 * building nodes. Enough for `GET /api/sessions` — id, title, cwd, model,
 * activity times and the sidechain→parent link.
 */
export const summarizeJsonl = (raw: string, source: ClaudeSourceInfo): Session =>
  sessionFrom(collectMeta(parseEntries(raw)), source, []);

/**
 * Read a `<session>.jsonl` transcript into IR. The session id is the file's
 * basename; the fallback cwd decodes the project dir name it sits in, and a
 * file under `<uuid>/subagents/` reports that uuid as its parent session.
 */
export const fromFile = (
  filePath: string,
  options?: { readonly id?: string; readonly parentSessionId?: string },
): Effect.Effect<Session, ConversionError, Fs.FileSystem | Path.Path> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const path = yield* Path.Path;

    const exists = yield* fs.exists(filePath);
    if (!exists) {
      return yield* Effect.fail(
        new ConversionError({
          message: `Claude Code transcript not found: ${filePath}`,
          cause: null,
        }),
      );
    }

    const raw = yield* fs.readFileString(filePath);
    const id = options?.id ?? path.basename(filePath).replace(/\.jsonl$/, "");
    const parentName = path.basename(path.dirname(filePath));
    const inSubagents = parentName === "subagents";
    const parentSessionId =
      options?.parentSessionId ??
      (inSubagents ? path.basename(path.dirname(path.dirname(filePath))) : undefined);
    const projectDirName = inSubagents
      ? path.basename(path.dirname(path.dirname(path.dirname(filePath))))
      : parentName;
    return fromJsonl(raw, { id, fallbackCwd: decodeProjectDir(projectDirName), parentSessionId });
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({
            message: `Claude Code conversion failed: ${String(error)}`,
            cause: error,
          }),
    ),
  );
