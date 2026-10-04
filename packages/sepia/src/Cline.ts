import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Console, Effect, Option } from "effect";
import { randomBytes, randomUUID } from "node:crypto";
import {
  ConversionError,
  MessageNode,
  PromptHistoryEntry,
  Session,
  ToolCall,
  type TokenUsage,
  type ToolResultInfo,
} from "./Domain.js";
import * as Devin from "./Devin.js";

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

const cleanUserText = (text: string): string => {
  if (text.startsWith("<user_input") && text.includes("</user_input>")) {
    const start = text.indexOf(">") + 1;
    const end = text.lastIndexOf("</user_input>");
    return text.slice(start, end).trim();
  }
  return text;
};

const keyOf = (k: unknown): string => {
  if (typeof k === "string") return k;
  if (Array.isArray(k)) {
    const parts = k.map((x) => String(x));
    return parts.length === 1 ? parts[0] : parts.join("\n");
  }
  return String(k);
};

/**
 * A list field in a Cline log is not always a list: the call is replayed as the
 * provider wrote it, so `commands`, `files`, `queries` and `requests` can arrive
 * as a JSON string (`"[\"a\", \"b\"]"`) or as a bare string when the model sent
 * one. Either way the items are still the ones to run.
 */
const asList = (value: unknown): ReadonlyArray<unknown> => {
  if (Array.isArray(value)) return value;
  if (typeof value !== "string") return [];
  const text = value.trim();
  if (text.startsWith("[")) {
    try {
      const parsed = JSON.parse(text);
      if (Array.isArray(parsed)) return parsed;
    } catch {}
  }
  return text.length > 0 ? [text] : [];
};

const makeToolCallId = (): string => `chatcmpl-tool-${randomUUID().replace(/-/g, "").slice(0, 16)}`;

/**
 * Sub-agent session ids embed their lineage: `<parent>__teamtask__<agent>__<rand>`
 * for team tasks and `<parent>__agent_<agent>` for spawned agents — the index
 * row's `parent_session_id`/`agent_id` repeat exactly these segments, so the
 * manifest-only read paths can recover them without opening `db/sessions.db`.
 */
export const clineSubagentInfo = (
  sessionId: string,
): { readonly parentSessionId: string; readonly agentId: string } | null => {
  // Nested team tasks chain the markers, and the parent is the session one
  // level up — so the split happens at the last `__teamtask__`, not the first.
  const teamtask = sessionId.lastIndexOf("__teamtask__");
  if (teamtask !== -1) {
    const rest = sessionId.slice(teamtask + "__teamtask__".length);
    const sep = rest.lastIndexOf("__");
    const agentId = sep === -1 ? rest : rest.slice(0, sep);
    if (agentId === "") return null;
    return { parentSessionId: sessionId.slice(0, teamtask), agentId };
  }
  const spawned = sessionId.indexOf("__agent_");
  if (spawned !== -1) {
    return {
      parentSessionId: sessionId.slice(0, spawned),
      agentId: sessionId.slice(spawned + 2),
    };
  }
  return null;
};

/**
 * The first field that carries the items of a call. Tool inputs are not uniform
 * across the sessions Cline has written: commands arrive as `commands` or as a
 * single `command`, reads as `files` or as `path` (+ line range).
 */
const firstList = (
  input: Record<string, unknown>,
  ...keys: ReadonlyArray<string>
): ReadonlyArray<unknown> => {
  for (const key of keys) {
    const items = asList(input[key]);
    if (items.length > 0) return items;
  }
  return [];
};

/** A list item is usually the text itself; a wrapper object still carries it. */
const asText = (value: unknown, ...keys: ReadonlyArray<string>): string => {
  if (typeof value === "string") return value;
  if (value !== null && typeof value === "object") {
    for (const key of keys) {
      const inner = (value as Record<string, unknown>)[key];
      if (typeof inner === "string") return inner;
    }
  }
  return value === undefined || value === null ? "" : JSON.stringify(value);
};

const mapToolUse = (
  clineTool: any,
): ReadonlyArray<{ name: string; arguments: unknown; resultKey: string }> => {
  const name = clineTool.name as string;
  const input = (clineTool.input ?? {}) as Record<string, unknown>;
  const calls: Array<{ name: string; arguments: unknown; resultKey: string }> = [];
  // A call whose items cannot be read is kept as it arrived: dropping it would
  // leave its result looking like output nobody asked for.
  const raw = () => calls.push({ name, arguments: input, resultKey: keyOf(input) });

  switch (name) {
    case "read_files": {
      const files = firstList(input, "files", "paths", "path", "file_path");
      if (files.length === 0) {
        raw();
        break;
      }
      for (const f of files) {
        const path = asText(f, "path", "file_path");
        calls.push({ name: "read", arguments: { file_path: path }, resultKey: keyOf(path) });
      }
      break;
    }
    case "run_commands": {
      const commands = firstList(input, "commands", "command", "cmd");
      if (commands.length === 0) {
        raw();
        break;
      }
      for (const cmd of commands) {
        const command = asText(cmd, "command");
        calls.push({ name: "exec", arguments: { command }, resultKey: keyOf(command) });
      }
      break;
    }
    case "search_codebase": {
      const queries = firstList(input, "queries", "query", "pattern");
      if (queries.length === 0) {
        raw();
        break;
      }
      for (const q of queries) {
        for (const pattern of Array.isArray(q) ? q : [q]) {
          const text = asText(pattern, "pattern", "query");
          calls.push({ name: "grep", arguments: { pattern: text }, resultKey: keyOf(text) });
        }
      }
      break;
    }
    case "fetch_web_content": {
      const requests = firstList(input, "requests", "url");
      if (requests.length === 0) {
        raw();
        break;
      }
      for (const req of requests) {
        const url = asText(req, "url");
        calls.push({ name: "webfetch", arguments: { url }, resultKey: keyOf(url) });
      }
      break;
    }
    case "editor": {
      const path = asText(input.path, "path");
      const oldText = input.old_text;
      const newText = input.new_text;
      if (path.length === 0) {
        raw();
      } else if (oldText === "null" || oldText === null || oldText === undefined) {
        calls.push({
          name: "write",
          arguments: { file_path: path, content: newText },
          resultKey: keyOf(path),
        });
      } else {
        calls.push({
          name: "edit",
          arguments: { file_path: path, old_string: oldText, new_string: newText },
          resultKey: keyOf(path),
        });
      }
      break;
    }
    default: {
      raw();
    }
  }

  return calls;
};

/**
 * The value each call's result should carry, aligned with `devinCalls`.
 *
 * Cline answers a multi-item call with one entry per item, in the order the
 * items were requested, but an entry's `query` is not always the text its call
 * carried: a read of a line range comes back as `path:start-end`, an array query
 * comes back joined, and a command can come back with different escaping. A key
 * match is therefore only a hint — entries that match no call fill the calls
 * that matched no entry, in order — so no result is silently dropped.
 */
/** One devin call's share of a Cline tool result: the text plus its success flag. */
interface ResultShare {
  readonly content: string | undefined;
  readonly success: boolean | undefined;
}

const toolResultShares = (
  clineResult: any,
  devinCalls: ReadonlyArray<{ resultKey: string }>,
): ReadonlyArray<ResultShare> => {
  const content = clineResult?.content;

  if (typeof content === "string") {
    return devinCalls.map(() => ({ content, success: undefined }));
  }
  if (!Array.isArray(content)) {
    return devinCalls.map(() => ({
      content: content === undefined || content === null ? undefined : JSON.stringify(content),
      success: undefined,
    }));
  }

  const entries = content.filter((item) => item !== undefined && item !== null);
  const values = entries.map((item: any) =>
    typeof item === "string"
      ? item
      : typeof item.result === "string"
        ? item.result
        : JSON.stringify(item.result ?? item),
  );
  const successes = entries.map((item: any) =>
    typeof item === "object" && typeof item.success === "boolean" ? item.success : undefined,
  );
  const keys = entries.map((item: any) =>
    typeof item === "object" ? keyOf(item.query ?? item.url ?? "") : "",
  );

  const claimed = new Set<number>();
  const assigned: Array<number | undefined> = devinCalls.map((call) => {
    const index = keys.findIndex((key, i) => key === call.resultKey && !claimed.has(i));
    if (index === -1) return undefined;
    claimed.add(index);
    return index;
  });

  const unclaimedEntries = entries.map((_, i) => i).filter((i) => !claimed.has(i));
  let next = 0;
  for (let i = 0; i < assigned.length && next < unclaimedEntries.length; i++) {
    if (assigned[i] === undefined) {
      assigned[i] = unclaimedEntries[next++];
    }
  }

  const out: Array<ResultShare> = assigned.map((index) =>
    index === undefined
      ? { content: undefined, success: undefined }
      : { content: values[index], success: successes[index] },
  );

  // Nothing a result carried is thrown away: entries no call could hold — a
  // tool that could not be split into calls, an answer with more parts than
  // calls — ride along on the last call, and a lone call keeps the whole answer.
  const tailIndices = unclaimedEntries.slice(next);
  const tail = tailIndices.map((i) => values[i]).join("\n");
  if (tail.length > 0 && out.length > 0) {
    const last = out.length - 1;
    const merged = out[last];
    const failed = merged.success === false || tailIndices.some((i) => successes[i] === false);
    out[last] = {
      content: merged.content === undefined ? tail : `${merged.content}\n${tail}`,
      success: merged.success === undefined && !failed ? undefined : failed ? false : true,
    };
  }

  return out;
};

const buildSystemNode = (
  nodeId: number,
  parentNodeId: Option.Option<number>,
  content: string,
  createdAt: number,
  isPrefix: boolean,
): MessageNode =>
  MessageNode.make({
    nodeId,
    parentNodeId,
    role: "system",
    content: sanitize(content),
    createdAt,
    metadata: {
      summarized_from: null,
      num_tokens_preceding: null,
      is_system_prefix: isPrefix,
    },
  });

const buildUserNode = (
  nodeId: number,
  parentNodeId: Option.Option<number>,
  text: string,
  createdAt: number,
): MessageNode =>
  MessageNode.make({
    nodeId,
    parentNodeId,
    role: "user",
    content: sanitize(text),
    createdAt,
    metadata: null,
  });

const buildAssistantNode = (
  nodeId: number,
  parentNodeId: Option.Option<number>,
  text: string,
  thinking: string,
  toolCalls: ReadonlyArray<ToolCall>,
  createdAt: number,
  rendered: boolean,
  usage: Option.Option<TokenUsage>,
  model: Option.Option<string>,
): MessageNode =>
  MessageNode.make({
    nodeId,
    parentNodeId,
    role: "assistant",
    content: sanitize(text),
    thinking: thinking ? Option.some(sanitize(thinking)) : Option.none<string>(),
    toolCalls,
    usage,
    model,
    createdAt,
    metadata: rendered
      ? { summarized_from: null, num_tokens_preceding: null, is_system_prefix: null }
      : null,
  });

const buildToolNode = (
  nodeId: number,
  parentNodeId: Option.Option<number>,
  toolCallId: string,
  content: string,
  toolName: string,
  toolArguments: unknown,
  createdAt: number,
  toolResult: Option.Option<ToolResultInfo>,
): MessageNode =>
  MessageNode.make({
    nodeId,
    parentNodeId,
    role: "tool",
    content: sanitize(content),
    toolCallId: Option.some(toolCallId),
    toolName: Option.some(toolName),
    toolResult,
    createdAt,
    metadata: toolArguments === null ? null : { toolArguments },
  });

const tsToEpochSeconds = (ts: unknown, fallback: number): number => {
  if (typeof ts === "number") {
    return ts > 1e12 ? Math.floor(ts / 1000) : ts;
  }
  return fallback;
};

/** Cline `metrics` uses camelCase token keys; `cost` may be absent. */
const usageFromClineMetrics = (metrics: unknown): Option.Option<TokenUsage> => {
  if (metrics === null || typeof metrics !== "object") return Option.none();
  const m = metrics as Record<string, unknown>;
  const num = (v: unknown): number | undefined =>
    typeof v === "number" && Number.isFinite(v) ? v : undefined;
  const input = num(m.inputTokens);
  const output = num(m.outputTokens);
  if (input === undefined && output === undefined) return Option.none();
  const cacheRead = num(m.cacheReadTokens);
  const cacheWrite = num(m.cacheWriteTokens);
  const cost = num(m.cost);
  return Option.some({
    input: input ?? 0,
    output: output ?? 0,
    ...(cacheRead !== undefined ? { cacheRead } : {}),
    ...(cacheWrite !== undefined ? { cacheWrite } : {}),
    ...(cost !== undefined ? { cost } : {}),
  });
};

const modelFromMessage = (m: any): Option.Option<string> => {
  const id = m?.modelInfo?.id;
  return typeof id === "string" && id !== "" ? Option.some(id) : Option.none<string>();
};

const parseMessagesData = (raw: unknown): { messages: ReadonlyArray<any> } => {
  if (raw && typeof raw === "object") {
    return { messages: Array.isArray((raw as any).messages) ? (raw as any).messages : [] };
  }
  return { messages: [] };
};

export const fromDirectory = (
  dir: string,
  sessionId?: string,
): Effect.Effect<Session, ConversionError, Fs.FileSystem | Path.Path> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const path = yield* Path.Path;

    const dirExists = yield* fs.exists(dir);
    if (!dirExists) {
      return yield* Effect.fail(
        new ConversionError({ message: `Cline session directory not found: ${dir}`, cause: null }),
      );
    }

    const entries = yield* fs.readDirectory(dir);
    const jsonFiles = entries.filter((name) => name.endsWith(".json"));
    const metaName = jsonFiles.find(
      (name) => !name.endsWith(".messages.json") && !name.includes(".compaction."),
    );

    if (!metaName) {
      return yield* Effect.fail(
        new ConversionError({ message: `No session metadata json found in ${dir}`, cause: null }),
      );
    }

    const base = metaName.replace(/\.json$/, "");
    const metaPath = path.join(dir, metaName);
    const messagesPath = path.join(dir, `${base}.messages.json`);

    const metaJson = yield* fs.readFileString(metaPath);
    const meta = JSON.parse(metaJson);

    // Sub-agent manifests point `messages_path` into the parent's directory;
    // the `<id>.messages.json` sibling only exists for root sessions.
    const resolvedMessagesPath =
      typeof meta.messages_path === "string" && meta.messages_path !== ""
        ? meta.messages_path
        : messagesPath;
    const messagesJson = yield* fs.readFileString(resolvedMessagesPath);
    const { messages } = parseMessagesData(JSON.parse(messagesJson));

    return yield* Effect.try({
      try: () => buildSession(dir, meta, messages, path, sessionId),
      catch: (error) =>
        new ConversionError({ message: `Failed to build session: ${String(error)}`, cause: error }),
    });
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({
            message: `Cline conversion failed: ${String(error)}`,
            cause: error,
          }),
    ),
  );

const buildSession = (
  dir: string,
  meta: any,
  messages: ReadonlyArray<any>,
  pathService: Path.Path,
  sessionId?: string,
): Session => {
  const cwd = meta.cwd ?? dir;
  const title = meta.metadata?.title ?? meta.prompt ?? "Imported session";
  const rawModel = meta.model ?? "glm-5-2";
  const model = rawModel.replace(/^cline-pass\//, "");
  const startedAt =
    typeof meta.started_at === "string"
      ? Math.floor(new Date(meta.started_at).getTime() / 1000)
      : Math.floor(Date.now() / 1000);
  const createdAt = startedAt;
  const lastActivityAt =
    typeof meta.ended_at === "string"
      ? Math.floor(new Date(meta.ended_at).getTime() / 1000)
      : Math.floor(Date.now() / 1000);

  const nodes: Array<MessageNode> = [];

  const addNode = (
    parent: Option.Option<number>,
    builder: (nid: number, p: Option.Option<number>) => MessageNode,
  ): number => {
    const node = builder(nodes.length, parent);
    nodes.push(node);
    return node.nodeId;
  };

  const sysInfo =
    `<system_info>\n` +
    `The following information is automatically generated context about your current environment.\n` +
    `Current workspace directories:\n  ${cwd} (cwd)\n\n` +
    `Platform: linux\n` +
    `</system_info>`;

  const n0 = addNode(Option.none<number>(), (nid, parent) =>
    buildSystemNode(nid, parent, sysInfo, createdAt, true),
  );
  const n1 = addNode(Option.some(n0), (nid, parent) =>
    buildSystemNode(nid, parent, `<rules type="always-on"></rules>`, createdAt, false),
  );

  const firstUserText = (() => {
    for (const m of messages) {
      if (m.role === "user") {
        for (const c of m.content ?? []) {
          if (c.type === "text" && c.text) {
            return cleanUserText(c.text);
          }
        }
      }
    }
    return "";
  })();

  const nUser = addNode(Option.some(n1), (nid, parent) =>
    buildUserNode(nid, parent, firstUserText, createdAt),
  );
  const nSkills = addNode(Option.some(nUser), (nid, parent) =>
    buildSystemNode(nid, parent, "<available_skills></available_skills>", createdAt, false),
  );

  let pendingToolCalls: Record<string, ReadonlyArray<{ devin: ToolCall; resultKey: string }>> = {};
  const toolCallOutcomes = new Map<string, Devin.ToolCallOutcome>();
  let lastRenderedAssistantNode = nSkills;
  let lastToolResultNode: number | null = null;
  let firstUserSeen = false;

  for (const m of messages) {
    const role = m.role as string;
    const ts = tsToEpochSeconds(m.ts, createdAt);

    if (role === "user") {
      const content = Array.isArray(m.content) ? m.content : [];
      const hasText = content.some((c: any) => c.type === "text");
      const hasToolResult = content.some((c: any) => c.type === "tool_result");

      if (hasText && !hasToolResult) {
        let text = "";
        for (const c of content) {
          if (c.type === "text" && c.text) {
            text = cleanUserText(c.text);
            break;
          }
        }

        if (text) {
          if (!firstUserSeen) {
            firstUserSeen = true;
            continue;
          }
          lastToolResultNode = null;
          const parent = Option.some(lastRenderedAssistantNode);
          addNode(parent, (nid, p) => buildUserNode(nid, p, text, ts));
          lastRenderedAssistantNode = nodes[nodes.length - 1].nodeId;
        }
      }

      if (hasToolResult) {
        for (const c of content) {
          if (c.type !== "tool_result") continue;
          const clineTuId = c.tool_use_id ?? "";
          const devinCalls = pendingToolCalls[clineTuId] ?? [];

          if (devinCalls.length === 0) {
            // Truly orphaned result: no assistant message in the log declares
            // this tool_use id (Cline-side compaction dropped the call).
            // Emitting a `tool` node would leave an unpaired tool_use_id that
            // the provider rejects, so keep the output as plain user text.
            const parent =
              lastToolResultNode !== null
                ? Option.some(lastToolResultNode)
                : Option.some(lastRenderedAssistantNode);
            const contentStr =
              typeof c.content === "string" ? c.content : JSON.stringify(c.content);
            addNode(parent, (nid, p) => buildUserNode(nid, p, `[tool output]\n${contentStr}`, ts));
            lastToolResultNode = nodes[nodes.length - 1].nodeId;
            continue;
          }

          const shares = toolResultShares(c, devinCalls);
          for (let i = 0; i < devinCalls.length; i++) {
            const { devin } = devinCalls[i];
            const share = shares[i];

            const status: ToolResultInfo["status"] = share.success === false ? "error" : "success";
            toolCallOutcomes.set(devin.id, { status });

            const parent =
              lastToolResultNode !== null
                ? Option.some(lastToolResultNode)
                : Option.some(lastRenderedAssistantNode);
            addNode(parent, (nid, p) =>
              buildToolNode(
                nid,
                p,
                devin.id,
                share.content ?? "",
                devin.name,
                devin.arguments,
                ts,
                Option.some({ status }),
              ),
            );
            lastToolResultNode = nodes[nodes.length - 1].nodeId;
          }

          delete pendingToolCalls[clineTuId];
        }
      }
    } else if (role === "assistant") {
      const content = Array.isArray(m.content) ? m.content : [];
      const textParts: Array<string> = [];
      let thinkingText = "";
      const toolUses: Array<any> = [];

      for (const c of content) {
        if (c.type === "text") textParts.push(c.text);
        if (c.type === "thinking") thinkingText = c.thinking ?? "";
        if (c.type === "tool_use") toolUses.push(c);
      }

      const text = textParts.join("\n");
      const devinToolCalls: Array<ToolCall> = [];
      const newPending: Record<string, ReadonlyArray<{ devin: ToolCall; resultKey: string }>> = {};

      for (const tu of toolUses) {
        const mapped = mapToolUse(tu);
        const callList: Array<{ devin: ToolCall; resultKey: string }> = [];
        for (const mc of mapped) {
          const tcId = makeToolCallId();
          const devinTc = ToolCall.make({
            id: tcId,
            name: mc.name,
            arguments: mc.arguments,
            index: devinToolCalls.length,
            kind: "function",
          });
          devinToolCalls.push(devinTc);
          callList.push({ devin: devinTc, resultKey: mc.resultKey });
        }
        newPending[tu.id] = callList;
      }

      // Calls from earlier turns stay claimable: an interrupted turn's output can
      // arrive after later turns, and pairing results by id keeps it with the
      // assistant turn that asked for it.
      pendingToolCalls = { ...pendingToolCalls, ...newPending };

      const parent =
        lastToolResultNode !== null
          ? Option.some(lastToolResultNode)
          : Option.some(lastRenderedAssistantNode);
      const usage = usageFromClineMetrics(m.metrics);
      const model = modelFromMessage(m);
      addNode(parent, (nid, p) =>
        buildAssistantNode(nid, p, text, thinkingText, devinToolCalls, ts, false, usage, model),
      );
      addNode(parent, (nid, p) =>
        buildAssistantNode(nid, p, text, thinkingText, devinToolCalls, ts, true, usage, model),
      );

      lastRenderedAssistantNode = nodes[nodes.length - 1].nodeId;
      lastToolResultNode = null;
    }
  }

  // Result entries carried the success flag per call — fold it back onto the
  // assistant twins' ToolCalls now that all results have been seen.
  const enrichedNodes = Devin.applyToolCallOutcomes(nodes, toolCallOutcomes);

  const mainChainId = enrichedNodes.length > 0 ? enrichedNodes[enrichedNodes.length - 1].nodeId : 0;

  const promptHistory: Array<PromptHistoryEntry> = [];
  for (const m of messages) {
    if (m.role === "user") {
      for (const c of m.content ?? []) {
        if (c.type === "text" && c.text) {
          promptHistory.push(
            PromptHistoryEntry.make({
              content: cleanUserText(c.text),
              timestamp: tsToEpochSeconds(m.ts, createdAt) * 1000,
              isShell: false,
            }),
          );
        }
      }
    }
  }

  const resolvedId = sessionId ?? meta.session_id ?? pathService.basename(dir);
  const subagent = clineSubagentInfo(resolvedId);

  return Session.make({
    id: resolvedId,
    title,
    workingDirectory: cwd,
    backendType: "windsurf",
    agentMode: "accept-edits",
    model,
    createdAt,
    lastActivityAt,
    mainChainId,
    shellLastSeenIndex: 0,
    cogsJson: Devin.defaultCogsJson(),
    workspaceDirs: "[]",
    hidden: 0,
    parentSessionId: Option.fromNullable(subagent?.parentSessionId),
    agentId: Option.fromNullable(subagent?.agentId),
    metadata: Devin.defaultSessionMetadata(),
    nodes: enrichedNodes,
    promptHistory,
  });
};

const toClineToolInput = (tc: ToolCall): unknown => {
  switch (tc.name) {
    case "read":
      return { files: [{ path: (tc.arguments as any)?.file_path }] };
    case "exec":
      return { commands: [(tc.arguments as any)?.command] };
    case "grep":
      return { queries: [(tc.arguments as any)?.pattern] };
    case "webfetch":
      return { requests: [{ url: (tc.arguments as any)?.url }] };
    case "edit":
      return {
        path: (tc.arguments as any)?.file_path,
        old_text: (tc.arguments as any)?.old_string,
        new_text: (tc.arguments as any)?.new_string,
      };
    case "write":
      return {
        path: (tc.arguments as any)?.file_path,
        old_text: null,
        new_text: (tc.arguments as any)?.content,
      };
    default:
      return tc.arguments;
  }
};

const toClineToolName = (name: string): string => {
  switch (name) {
    case "read":
      return "read_files";
    case "exec":
      return "run_commands";
    case "grep":
      return "search_codebase";
    case "webfetch":
      return "fetch_web_content";
    case "edit":
    case "write":
      return "editor";
    default:
      return name;
  }
};

const toClineToolResultContent = (node: MessageNode, toolName: string): unknown => {
  if (
    toolName !== "read_files" &&
    toolName !== "run_commands" &&
    toolName !== "search_codebase" &&
    toolName !== "fetch_web_content"
  ) {
    return node.content;
  }

  const args =
    node.metadata !== null &&
    node.metadata !== undefined &&
    typeof node.metadata === "object" &&
    "toolArguments" in node.metadata
      ? (node.metadata as { toolArguments: unknown }).toolArguments
      : null;

  let query = "";
  if (args !== null && typeof args === "object") {
    query =
      (args as { file_path?: string }).file_path ??
      (args as { command?: string }).command ??
      (args as { pattern?: string }).pattern ??
      (args as { url?: string }).url ??
      "";
  }

  // A call whose arguments carry none of these keys has no query to name it by;
  // the output then stands on its own rather than next to an empty label.
  if (query.length === 0) {
    return node.content;
  }

  const result = Option.getOrUndefined(node.toolResult);
  return [
    {
      query,
      result: node.content,
      success: result === undefined ? true : result.status !== "error",
    },
  ];
};

const assistantFingerprint = (node: MessageNode): string =>
  JSON.stringify([
    Option.getOrUndefined(node.parentNodeId) ?? null,
    node.content,
    Option.getOrUndefined(node.thinking) ?? "",
    node.toolCalls.map((tc) => tc.id),
  ]);

/**
 * Devin records an imported assistant turn twice: an unrendered twin (no row
 * metadata) and a rendered twin (row metadata carries token counts and tool
 * display info). Cline wants exactly one assistant turn per source message, so
 * keep the first node of each twin pair and every assistant that has no twin.
 * System nodes are dropped: Cline rebuilds its own system prompt on resume.
 */
export const visibleNodes = (session: Session): ReadonlyArray<MessageNode> => {
  const seen = new Set<string>();
  return session.nodes.filter((node) => {
    if (node.role === "system") return false;
    if (node.role !== "assistant") return true;
    const key = assistantFingerprint(node);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
};

export const CLINE_PROVIDER = "cline-pass";
export const CLINE_AGENT_VERSION = "3.0.61";
const CLINE_ID_ALPHABET = "abcdefghijklmnopqrstuvwxyz0123456789";

/**
 * Cline session ids are `<epoch-ms>_<5 random alphanumerics>`; the id doubles
 * as the session directory name and as the prefix of both artifacts in it.
 */
export const clineSessionId = (atMs: number = Date.now()): string => {
  let suffix = "";
  for (const byte of randomBytes(5)) {
    suffix += CLINE_ID_ALPHABET[byte % CLINE_ID_ALPHABET.length];
  }
  return `${atMs}_${suffix}`;
};

/**
 * Manifest for a Cline session: the field set the CLI's SessionManifest schema
 * requires in `<session-id>.json`, so the session can be resumed by id.
 */
export const sessionManifest = (
  session: Session,
  sessionId: string,
  messagesPath: string,
): Record<string, unknown> => ({
  version: 1,
  session_id: sessionId,
  source: "cli",
  pid: 0,
  cwd: session.workingDirectory,
  workspace_root: session.workingDirectory,
  started_at: new Date(session.createdAt * 1000).toISOString(),
  ended_at: new Date(session.lastActivityAt * 1000).toISOString(),
  status: "completed",
  exit_code: 0,
  interactive: true,
  provider: CLINE_PROVIDER,
  model: session.model,
  enable_tools: true,
  enable_spawn: true,
  enable_teams: true,
  prompt: session.nodes.find((n) => n.role === "user")?.content ?? "",
  metadata: { title: session.title },
  messages_path: messagesPath,
});

/**
 * The placeholder the CLI's own import path (`sanitizeImportedMessages`) writes
 * for a tool call whose result the source log never captured. Sepia writes the
 * same block, so an imported log is what the CLI itself would have produced.
 */
export const UNCAPTURED_TOOL_RESULT =
  "[import] Tool result was not captured in the source session history.";

const toolResultBlock = (node: MessageNode, name: string): unknown => ({
  type: "tool_result",
  tool_use_id: Option.getOrElse(node.toolCallId, () => ""),
  name,
  content: toClineToolResultContent(node, name),
});

const uncapturedToolResultBlock = (id: string, name: string): unknown => ({
  type: "tool_result",
  tool_use_id: id,
  name,
  content: UNCAPTURED_TOOL_RESULT,
});

/**
 * Message log for a Cline session, in the CLI's `<session-id>.messages.json` shape.
 *
 * The CLI replays the log through the AI SDK, which rejects the transcript as
 * soon as a user turn follows an assistant tool call that is still unresolved
 * (`AI_MissingToolResultsError`). Nodes are therefore not emitted in raw order:
 * a call is paired with its result by tool-call id, results are written directly
 * behind the assistant turn that made the calls, and a call no result was ever
 * recorded for gets a placeholder result.
 */
export const sessionMessages = (session: Session, sessionId: string): Record<string, unknown> => {
  const messages: Array<unknown> = [];
  const relevantNodes = visibleNodes(session);

  const callsByAssistant = new Map<number, ReadonlyArray<{ id: string; name: string }>>();
  const assistantByCallId = new Map<string, number>();
  for (const node of relevantNodes) {
    if (node.role !== "assistant" || node.toolCalls.length === 0) continue;
    callsByAssistant.set(
      node.nodeId,
      node.toolCalls.map((tc) => ({ id: tc.id, name: toClineToolName(tc.name) })),
    );
    for (const tc of node.toolCalls) {
      assistantByCallId.set(tc.id, node.nodeId);
    }
  }

  const resultsByCallId = new Map<string, MessageNode>();
  for (const node of relevantNodes) {
    const callId = Option.getOrUndefined(node.toolCallId);
    if (node.role === "tool" && callId !== undefined && assistantByCallId.has(callId)) {
      resultsByCallId.set(callId, node);
    }
  }

  let messageIndex = 0;
  const nextId = () => `msg_${messageIndex++}`;

  for (const node of relevantNodes) {
    if (node.role === "user") {
      messages.push({
        id: nextId(),
        role: "user",
        content: [{ type: "text", text: node.content }],
        ts: node.createdAt * 1000,
      });
      continue;
    }

    if (node.role === "assistant") {
      const content: Array<unknown> = [];
      if (node.content) content.push({ type: "text", text: node.content });
      if (Option.isSome(node.thinking)) {
        content.push({ type: "thinking", thinking: node.thinking.value });
      }
      for (const tc of node.toolCalls) {
        content.push({
          type: "tool_use",
          id: tc.id,
          name: toClineToolName(tc.name),
          input: toClineToolInput(tc),
        });
      }
      // A node with nothing in it carries nothing: the CLI's own import path
      // drops such a turn, so sepia does not write one either.
      if (content.length === 0) continue;
      const usage = Option.getOrUndefined(node.usage);
      messages.push({
        id: nextId(),
        role: "assistant",
        content,
        ts: node.createdAt * 1000,
        modelInfo: {
          id: Option.getOrElse(node.model, () => session.model),
          provider: CLINE_PROVIDER,
        },
        metrics: {
          inputTokens: usage?.input ?? 0,
          outputTokens: usage?.output ?? 0,
          cacheReadTokens: usage?.cacheRead ?? 0,
          cacheWriteTokens: usage?.cacheWrite ?? 0,
          cost: usage?.cost ?? 0,
        },
      });

      const calls = callsByAssistant.get(node.nodeId);
      if (calls !== undefined) {
        messages.push({
          id: nextId(),
          role: "user",
          content: calls.map((call) => {
            const result = resultsByCallId.get(call.id);
            if (result === undefined) {
              return uncapturedToolResultBlock(call.id, call.name);
            }
            const name = toClineToolName(Option.getOrElse(result.toolName, () => call.name));
            return toolResultBlock(result, name);
          }),
          ts: node.createdAt * 1000 + 1,
        });
      }
      continue;
    }

    // A result no visible assistant turn claims cannot be paired with a call:
    // keep it as its own block, which the CLI's log format tolerates. Results a
    // visible call owns were already written behind their assistant turn.
    const callId = Option.getOrUndefined(node.toolCallId);
    if (callId !== undefined && assistantByCallId.has(callId)) {
      continue;
    }
    const toolName = toClineToolName(Option.getOrElse(node.toolName, () => "unknown"));
    messages.push({
      id: nextId(),
      role: "user",
      content: [toolResultBlock(node, toolName)],
      ts: node.createdAt * 1000,
    });
  }

  return {
    version: 1,
    updated_at: new Date(session.lastActivityAt * 1000).toISOString(),
    agent: "lead",
    sessionId,
    origin: { source: "cli", mode: "user", sessionId, version: CLINE_AGENT_VERSION },
    messages,
  };
};

/**
 * The rule the Cline CLI enforces when it replays a session: a turn that is not
 * a tool result must not arrive while an assistant tool call is unresolved — the
 * AI SDK raises `AI_MissingToolResultsError`, and the CLI reports it as "tool
 * results are missing" — and no call may stay unresolved at the end of the log.
 * A transcript `sessionMessages` produced reports no violation here.
 */
export const transcriptViolations = (
  messages: ReadonlyArray<unknown>,
): ReadonlyArray<{
  readonly index: number | "eof";
  readonly toolCallIds: ReadonlyArray<string>;
}> => {
  const blocksOf = (message: any): ReadonlyArray<any> =>
    Array.isArray(message?.content)
      ? message.content.filter((block: unknown) => typeof block === "object" && block !== null)
      : [];

  const pending = new Set<string>();
  const violations: Array<{
    readonly index: number | "eof";
    readonly toolCallIds: ReadonlyArray<string>;
  }> = [];

  messages.forEach((message: any, index) => {
    if (message?.role === "assistant") {
      for (const block of blocksOf(message)) {
        if (block.type === "tool_use" && typeof block.id === "string") {
          pending.add(block.id);
        }
      }
      return;
    }

    const blocks = blocksOf(message);
    const results = blocks.filter((block) => block.type === "tool_result");
    const resultOnly = results.length > 0 && results.length === blocks.length;
    if (!resultOnly && pending.size > 0) {
      violations.push({ index, toolCallIds: [...pending].sort() });
    }
    for (const block of results) {
      pending.delete(block.tool_use_id);
    }
  });

  if (pending.size > 0) {
    violations.push({ index: "eof", toolCallIds: [...pending].sort() });
  }

  return violations;
};

/** Writes the `<id>.json` manifest and `<id>.messages.json` transcript pair. */
export const writeSessionFiles = (
  fs: Fs.FileSystem,
  sessionId: string,
  outDir: string,
  manifest: Record<string, unknown>,
  messages: Record<string, unknown>,
): Effect.Effect<void, ConversionError> =>
  Effect.gen(function* () {
    yield* fs.writeFileString(`${outDir}/${sessionId}.json`, JSON.stringify(manifest, null, 2));
    yield* fs.writeFileString(
      `${outDir}/${sessionId}.messages.json`,
      JSON.stringify(messages, null, 2),
    );
  }).pipe(
    Effect.mapError(
      (error) =>
        new ConversionError({ message: `Cline export failed: ${String(error)}`, cause: error }),
    ),
  );

/** Reads an existing export back; a missing manifest means nothing to replace. */
export const readSessionFiles = (
  fs: Fs.FileSystem,
  sessionId: string,
  outDir: string,
): Effect.Effect<
  Option.Option<{ readonly manifest: string; readonly messages: string }>,
  ConversionError
> =>
  Effect.gen(function* () {
    const manifestPath = `${outDir}/${sessionId}.json`;
    const exists = yield* fs.exists(manifestPath).pipe(Effect.orElseSucceed(() => false));
    if (!exists) return Option.none();
    const manifest = yield* fs.readFileString(manifestPath);
    const messages = yield* fs
      .readFileString(`${outDir}/${sessionId}.messages.json`)
      .pipe(Effect.orElseSucceed(() => "{}"));
    return Option.some({ manifest, messages });
  }).pipe(
    Effect.mapError(
      (error) =>
        new ConversionError({ message: `Cline export failed: ${String(error)}`, cause: error }),
    ),
  );

/**
 * Export a Devin session to a directory as Cline session files (`<id>.json`
 * plus `<id>.messages.json`). Existing files are never replaced: without
 * `force` the export reports them and leaves them untouched, which makes
 * re-running an export idempotent. With `dryRun` it only validates and reports.
 */
export const toDirectory = (
  session: Session,
  outDir: string,
  options?: { readonly force?: boolean; readonly dryRun?: boolean },
): Effect.Effect<
  ReadonlyArray<"created" | "kept" | "replaced" | "planned">,
  ConversionError,
  Fs.FileSystem
> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;

    const messagesPath = `${outDir}/${session.id}.messages.json`;
    const previous = yield* readSessionFiles(fs, session.id, outDir);
    if (Option.isSome(previous) && options?.force !== true) {
      yield* Console.log(`Session ${session.id} already exists in ${outDir}; leaving it untouched`);
      return ["kept", "kept"] as const;
    }

    if (options?.dryRun === true) {
      yield* Console.log(`Would export session ${session.id} to ${outDir}`);
      return ["planned", "planned"] as const;
    }

    yield* fs.makeDirectory(outDir, { recursive: true });
    yield* writeSessionFiles(
      fs,
      session.id,
      outDir,
      sessionManifest(session, session.id, messagesPath),
      sessionMessages(session, session.id),
    );

    if (Option.isSome(previous)) {
      return ["replaced", "replaced"] as const;
    }
    return ["created", "created"] as const;
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Cline export failed: ${String(error)}`, cause: error }),
    ),
  );
