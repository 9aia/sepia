import { Console, Effect, Option, Schema } from "effect";
import * as Cline from "./Cline.js";
import { ClineStore } from "./ClineStore.js";
import * as Devin from "./Devin.js";
import {
  Block,
  CheckpointRef,
  ConversionError,
  MessageNode,
  PromptHistoryEntry,
  Role,
  Session,
  ToolCall,
  ToolCallDiff,
  ToolCallLocation,
  ToolResultInfo,
  TokenUsage,
  ToolCallStatus,
} from "./Domain.js";
import { SessionRepository } from "./Storage.js";

/** A cogs blob is usable when its `core/model` cog resolves to a real model. */
const modelCog = (cogsJson: string): unknown => {
  try {
    const cogs = JSON.parse(cogsJson);
    if (!Array.isArray(cogs)) return null;
    return (
      cogs.find(
        (c) =>
          c?.lifetime?.Unique === "core/model" &&
          typeof c?.model === "string" &&
          c.model.length > 0,
      ) ?? null
    );
  } catch {
    return null;
  }
};

/** Fill the stub `core/model` cog's model when no donor session exists. */
const withModel = (cogsJson: string, model: string): string => {
  try {
    const cogs = JSON.parse(cogsJson);
    if (Array.isArray(cogs)) {
      for (const c of cogs) {
        if (c?.lifetime?.Unique === "core/model") c.model = model;
      }
      return JSON.stringify(cogs);
    }
  } catch {}
  return cogsJson;
};

// Sepia cannot mint Devin's cog scaffolding (model cog, tool allow-list,
// profile prompt), so graft it from a sibling session in the store —
// same working directory preferred — or at least fill the model cog.
const graftCogs = (session: Session) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const donors = yield* repo.list().pipe(Effect.orElseSucceed(() => [] as const));
    const donor =
      donors.find(
        (s) =>
          s.id !== session.id &&
          s.workingDirectory === session.workingDirectory &&
          modelCog(s.cogsJson),
      ) ?? donors.find((s) => s.id !== session.id && modelCog(s.cogsJson));
    if (donor) {
      yield* Console.log(`Grafted cogs from session ${donor.id}`);
      return donor.cogsJson;
    }
    yield* Console.log("Warning: no donor session found to graft cogs from; resume may fail");
    return withModel(session.cogsJson, session.model);
  });

/**
 * Save a session into the Devin store, grafting cog scaffolding from a donor
 * session when the session's own cogs carry no usable model. Sessions that
 * already exist are left untouched. Shared by `importCline` (Cline dir →
 * Devin store) and `POST /api/sessions/import` (explicit IR → Devin store).
 */
export const importSession = (session: Session, importedLog?: string) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const exists = yield* repo.hasSession(session.id);
    if (exists) {
      yield* Console.log(`Session ${session.id} is already imported; leaving it untouched`);
      return session.id;
    }

    const cogsJson = yield* graftCogs(session);
    yield* repo.save(
      Session.make({
        id: session.id,
        title: session.title,
        workingDirectory: session.workingDirectory,
        backendType: session.backendType,
        agentMode: session.agentMode,
        model: session.model,
        createdAt: session.createdAt,
        lastActivityAt: session.lastActivityAt,
        mainChainId: session.mainChainId,
        shellLastSeenIndex: session.shellLastSeenIndex,
        cogsJson,
        workspaceDirs: session.workspaceDirs,
        hidden: session.hidden,
        parentSessionId: session.parentSessionId,
        agentId: session.agentId,
        checkpoints: session.checkpoints,
        metadata: session.metadata,
        nodes: session.nodes,
        promptHistory: session.promptHistory,
      }),
    );
    yield* Console.log(importedLog ?? `Imported session ${session.id} into storage`);
    return session.id;
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Import failed: ${String(error)}`, cause: error }),
    ),
  );

export const importCline = (
  clineDir: string,
  sessionId?: string,
  options?: { readonly dryRun?: boolean },
) =>
  Effect.gen(function* () {
    const session = yield* Cline.fromDirectory(clineDir, sessionId);
    const repo = yield* SessionRepository;

    const storedId = session.id;
    const exists = yield* repo.hasSession(storedId);
    if (exists) {
      yield* Console.log(`Session ${storedId} is already imported; leaving it untouched`);
      return storedId;
    }

    if (options?.dryRun === true) {
      yield* Console.log(`Would import Cline session ${storedId} into storage`);
      return storedId;
    }

    return yield* importSession(session, `Imported Cline session ${session.id} into storage`);
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Import failed: ${String(error)}`, cause: error }),
    ),
  );

export const exportCline = (
  sessionId: string,
  outDir: string,
  options?: { readonly force?: boolean; readonly dryRun?: boolean },
) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const opt = yield* repo.getById(sessionId);
    if (Option.isNone(opt)) {
      return yield* Effect.fail(
        new ConversionError({ message: `Session not found: ${sessionId}`, cause: null }),
      );
    }
    yield* Cline.toDirectory(opt.value, outDir, options);
    yield* Console.log(`Exported session ${sessionId} to ${outDir}`);
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Export failed: ${String(error)}`, cause: error }),
    ),
  );

/**
 * Export a Devin session straight into the Cline CLI store: artifacts land in
 * `<dataDir>/sessions/<session-id>/` and the session index is updated, so the
 * session shows up in `cline history` and resumes with `cline --id <session-id>`.
 */
export const installCline = (
  sessionId: string,
  dataDir: string,
  newSessionId?: string,
  options?: { readonly force?: boolean },
) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const store = yield* ClineStore;

    const opt = yield* repo.getById(sessionId);
    if (Option.isNone(opt)) {
      return yield* Effect.fail(
        new ConversionError({ message: `Session not found: ${sessionId}`, cause: null }),
      );
    }

    const installed = yield* store.install(opt.value, newSessionId, options);
    yield* Console.log(`Installed session ${installed} into ${dataDir}`);
    yield* Console.log(`Resume it with: cline --id ${installed} -m <model>`);
    return installed;
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Install failed: ${String(error)}`, cause: error }),
    ),
  );

export const listSessions = () =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const sessions = yield* repo.list();
    if (sessions.length === 0) {
      yield* Console.log("No sessions found");
    } else {
      for (const s of sessions) {
        yield* Console.log(`${s.id}\t${s.title}\t${s.workingDirectory}`);
      }
    }
  });

/** One flattened IR message — the wire shape of `GET /api/sessions/:id/history`. */
export interface ImportedHistoryMessage {
  readonly role: "system" | "user" | "assistant" | "tool";
  readonly content: string;
  /**
   * Non-text content the message carried (images, attachments) plus its text
   * blocks — present only when the store recorded them; `content` alone is
   * the whole message otherwise.
   */
  readonly blocks?: ReadonlyArray<Block>;
  /** Epoch milliseconds. */
  readonly createdAt: number;
  readonly toolName?: string;
  /** Display text of the message's reasoning, when the store recorded any. */
  readonly thinking?: string;
  /** Opaque provider seal on `thinking` — never decoded, replayed verbatim. */
  readonly thinkingSignature?: string;
  readonly usage?: TokenUsage;
  readonly model?: string;
  readonly requestId?: string;
  readonly finishReason?: string;
  /** Tool-result nodes only: how the call this message answers ended. */
  readonly toolStatus?: ToolCallStatus;
  readonly exitCode?: number;
  readonly durationMs?: number;
}

const historyMessageSeconds = (ms: number, fallback: number): number =>
  Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : fallback;

/**
 * Rebuild a `Session` IR from the flattened message list `GET
 * /api/sessions/:id/history` serves. The projection drops tool-call ids, so
 * nodes are re-chained linearly — what survives (roles, text, thinking and
 * its signature, tool output, timestamps) is exactly what a portable resume
 * needs. The result feeds the same write paths as `importCline`/`installCline`.
 */
export const sessionFromHistory = (input: {
  readonly id: string;
  readonly title: string;
  readonly cwd: string;
  readonly model: string;
  readonly history: ReadonlyArray<ImportedHistoryMessage>;
}): Session => {
  const now = Math.floor(Date.now() / 1000);
  const nodes = input.history.map((message, index) =>
    MessageNode.make({
      nodeId: index,
      parentNodeId: index === 0 ? Option.none<number>() : Option.some(index - 1),
      role: message.role,
      content: message.content,
      ...(message.blocks === undefined ? {} : { blocks: message.blocks }),
      createdAt: historyMessageSeconds(message.createdAt, now),
      toolName:
        typeof message.toolName === "string" && message.toolName !== ""
          ? Option.some(message.toolName)
          : Option.none<string>(),
      thinking:
        typeof message.thinking === "string" && message.thinking !== ""
          ? Option.some(message.thinking)
          : Option.none<string>(),
      thinkingSignature:
        typeof message.thinkingSignature === "string"
          ? Option.some(message.thinkingSignature)
          : Option.none<string>(),
      usage: message.usage === undefined ? Option.none() : Option.some(message.usage),
      model: typeof message.model === "string" ? Option.some(message.model) : Option.none<string>(),
      requestId:
        typeof message.requestId === "string"
          ? Option.some(message.requestId)
          : Option.none<string>(),
      finishReason:
        typeof message.finishReason === "string"
          ? Option.some(message.finishReason)
          : Option.none<string>(),
      toolResult:
        message.toolStatus === undefined
          ? Option.none()
          : Option.some({
              status: message.toolStatus,
              ...(message.exitCode === undefined ? {} : { exitCode: message.exitCode }),
              ...(message.durationMs === undefined ? {} : { durationMs: message.durationMs }),
            }),
      metadata:
        message.role === "assistant"
          ? { summarized_from: null, num_tokens_preceding: null, is_system_prefix: null }
          : message.role === "system"
            ? { summarized_from: null, num_tokens_preceding: null, is_system_prefix: index === 0 }
            : null,
    }),
  );
  const createdAt = nodes[0]?.createdAt ?? now;
  const lastActivityAt = nodes[nodes.length - 1]?.createdAt ?? createdAt;
  const promptHistory = input.history.flatMap((message) =>
    message.role === "user"
      ? [
          PromptHistoryEntry.make({
            content: message.content,
            timestamp: Number.isFinite(message.createdAt) ? message.createdAt : now * 1000,
            isShell: false,
          }),
        ]
      : [],
  );
  return Session.make({
    id: input.id,
    title: input.title,
    workingDirectory: input.cwd,
    backendType: "windsurf",
    agentMode: "accept-edits",
    model: input.model,
    createdAt,
    lastActivityAt,
    mainChainId: nodes.length > 0 ? nodes.length - 1 : 0,
    shellLastSeenIndex: 0,
    cogsJson: Devin.defaultCogsJson(),
    workspaceDirs: "[]",
    hidden: 0,
    metadata: Devin.defaultSessionMetadata(),
    nodes,
    promptHistory,
  });
};

/**
 * JSON wire shape of the full session IR — the `session` payload served by
 * `GET /api/sessions/:id/export` and accepted by `POST /api/sessions/import`.
 * `Option` fields ride as `field?: value` (absent means none) so the payload
 * is plain JSON; unlike the flattened history projection this keeps
 * tool-call ids/args, thinking, per-node usage and the parent-linked tree.
 */
const ToolCallJson = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  arguments: Schema.Unknown,
  index: Schema.optionalWith(Schema.Number, { default: () => 0 }),
  kind: Schema.optionalWith(Schema.String, { default: () => "function" }),
  status: Schema.OptionFromUndefinedOr(ToolCallStatus),
  exitCode: Schema.OptionFromUndefinedOr(Schema.Number),
  durationMs: Schema.OptionFromUndefinedOr(Schema.Number),
  locations: Schema.optionalWith(Schema.Array(ToolCallLocation), { default: () => [] }),
  diffs: Schema.optionalWith(Schema.Array(ToolCallDiff), { default: () => [] }),
});

const MessageNodeJson = Schema.Struct({
  nodeId: Schema.Number,
  parentNodeId: Schema.OptionFromUndefinedOr(Schema.Number),
  role: Role,
  content: Schema.String,
  blocks: Schema.optionalWith(Schema.Array(Block), { default: () => [] }),
  toolCalls: Schema.optionalWith(Schema.Array(ToolCallJson), { default: () => [] }),
  toolCallId: Schema.OptionFromUndefinedOr(Schema.String),
  toolName: Schema.OptionFromUndefinedOr(Schema.String),
  thinking: Schema.OptionFromUndefinedOr(Schema.String),
  thinkingSignature: Schema.OptionFromUndefinedOr(Schema.String),
  usage: Schema.OptionFromUndefinedOr(TokenUsage),
  model: Schema.OptionFromUndefinedOr(Schema.String),
  requestId: Schema.OptionFromUndefinedOr(Schema.String),
  finishReason: Schema.OptionFromUndefinedOr(Schema.String),
  toolResult: Schema.OptionFromUndefinedOr(ToolResultInfo),
  createdAt: Schema.Number,
  metadata: Schema.Unknown,
});

const PromptHistoryJson = Schema.Struct({
  content: Schema.String,
  timestamp: Schema.Number,
  isShell: Schema.optionalWith(Schema.Boolean, { default: () => false }),
});

export const SessionJson = Schema.Struct({
  id: Schema.String,
  title: Schema.String,
  workingDirectory: Schema.String,
  backendType: Schema.optionalWith(Schema.String, { default: () => "windsurf" }),
  agentMode: Schema.optionalWith(Schema.String, { default: () => "accept-edits" }),
  model: Schema.String,
  createdAt: Schema.Number,
  lastActivityAt: Schema.Number,
  mainChainId: Schema.Number,
  shellLastSeenIndex: Schema.optionalWith(Schema.Number, { default: () => 0 }),
  cogsJson: Schema.optionalWith(Schema.String, { default: () => "[]" }),
  workspaceDirs: Schema.optionalWith(Schema.String, { default: () => "[]" }),
  hidden: Schema.optionalWith(Schema.Number, { default: () => 0 }),
  parentSessionId: Schema.OptionFromUndefinedOr(Schema.String),
  agentId: Schema.OptionFromUndefinedOr(Schema.String),
  checkpoints: Schema.optionalWith(Schema.Array(CheckpointRef), { default: () => [] }),
  metadata: Schema.Unknown,
  nodes: Schema.optionalWith(Schema.Array(MessageNodeJson), { default: () => [] }),
  promptHistory: Schema.optionalWith(Schema.Array(PromptHistoryJson), { default: () => [] }),
});

export type SessionJson = Schema.Schema.Encoded<typeof SessionJson>;

/** Encode a full Session IR into the `session` wire payload of `/export`. */
export const sessionToJson = (session: Session): SessionJson =>
  Schema.encodeSync(SessionJson)(session);

/**
 * Decode a `/export` wire payload back into a `Session` — the faithful
 * counterpart of `sessionFromHistory`, preserving toolCalls ids/args,
 * thinking, usage, toolCallId links and `parentNodeId` structure. Throws a
 * `ParseError` when the payload is not a session IR; callers map that to a
 * client error.
 */
export const sessionFromJson = (input: unknown): Session => {
  const decoded = Schema.decodeUnknownSync(SessionJson)(input);
  return Session.make({
    ...decoded,
    nodes: decoded.nodes.map((node) =>
      MessageNode.make({
        ...node,
        toolCalls: node.toolCalls.map((call) => ToolCall.make(call)),
      }),
    ),
    promptHistory: decoded.promptHistory.map((entry) => PromptHistoryEntry.make(entry)),
  });
};
