import { Option, Schema } from "effect";

export const Role = Schema.Literal("system", "user", "assistant", "tool");

/** Lifecycle of a tool call as the agent's store recorded it. */
export const ToolCallStatus = Schema.Literal("pending", "success", "error");
export type ToolCallStatus = Schema.Schema.Type<typeof ToolCallStatus>;

/**
 * Token metrics for one message. `input`/`output` are the counts every store
 * carries; the rest appear only where the agent persists them (Cline `cost`,
 * provider thinking-token counters).
 */
export const TokenUsage = Schema.Struct({
  input: Schema.Number,
  output: Schema.Number,
  cacheRead: Schema.optional(Schema.Number),
  cacheWrite: Schema.optional(Schema.Number),
  thinking: Schema.optional(Schema.Number),
  cost: Schema.optional(Schema.Number),
});
export type TokenUsage = Schema.Schema.Type<typeof TokenUsage>;

/** Outcome a `role: "tool"` node reports back for the call it answers. */
export const ToolResultInfo = Schema.Struct({
  status: ToolCallStatus,
  exitCode: Schema.optional(Schema.Number),
  durationMs: Schema.optional(Schema.Number),
});
export type ToolResultInfo = Schema.Schema.Type<typeof ToolResultInfo>;

/**
 * A file location a tool call touched — the ACP `locations` entries Devin
 * persists (`{path, line?}`) and the paths Cline tool inputs name.
 */
export const ToolCallLocation = Schema.Struct({
  path: Schema.String,
  line: Schema.optional(Schema.Number),
});
export type ToolCallLocation = Schema.Schema.Type<typeof ToolCallLocation>;

/**
 * The before/after payload a store recorded for a file change — ACP `diff`
 * tool-call content (`{path, oldText?, newText?}`) on Devin, `editor` tool
 * inputs on Cline (`old_text`/`new_text`; absent `oldText` means a create,
 * absent `newText` a delete — a write-only diff is still a diff).
 */
export const ToolCallDiff = Schema.Struct({
  path: Schema.String,
  oldText: Schema.optional(Schema.String),
  newText: Schema.optional(Schema.String),
});
export type ToolCallDiff = Schema.Schema.Type<typeof ToolCallDiff>;

/**
 * A snapshot pointer a store records for workspace state — a reference,
 * never a payload. Cline keeps them in the session manifest's
 * `metadata.checkpoint` (`{ref, createdAt, runCount, kind}`: shadow-git
 * stash/commit shas in the workspace repo); Claude's `file-history-snapshot`
 * and Cursor's `originalFileStates` could map onto the same shape.
 * `createdAt` is epoch **milliseconds** (the store-native unit).
 */
export const CheckpointRef = Schema.Struct({
  ref: Schema.String,
  createdAt: Schema.Number,
  runCount: Schema.optional(Schema.Number),
  kind: Schema.optional(Schema.String),
});
export type CheckpointRef = Schema.Schema.Type<typeof CheckpointRef>;

export class ToolCall extends Schema.Class<ToolCall>("ToolCall")({
  id: Schema.String,
  name: Schema.String,
  arguments: Schema.Unknown,
  index: Schema.Number.pipe(Schema.optionalWith({ default: () => 0 })),
  kind: Schema.String.pipe(Schema.optionalWith({ default: () => "function" })),
  status: Schema.OptionFromSelf(ToolCallStatus).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  exitCode: Schema.OptionFromSelf(Schema.Number).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  durationMs: Schema.OptionFromSelf(Schema.Number).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /** Files the call touched, when the store recorded them (ACP `locations`). */
  locations: Schema.Array(ToolCallLocation).pipe(Schema.optionalWith({ default: () => [] })),
  /**
   * File changes the call made, when the store recorded before/after
   * payloads (Devin's ACP `diff` content, Cline `editor` inputs).
   */
  diffs: Schema.Array(ToolCallDiff).pipe(Schema.optionalWith({ default: () => [] })),
}) {}

/**
 * One piece of message content beyond the flat `content` string — the shape
 * mirrors what the stores actually carry: Devin persists the ACP
 * `ContentBlock[]` a prompt was sent with under
 * `metadata.extensions["chisel/acp-content-blocks"]`, and Cline/Claude-style
 * transcripts keep `image`/`document` entries in the message `content` array.
 *
 * When `blocks` is populated it holds the complete ordered block list —
 * text blocks included — so `content` stays the joined text projection and
 * writes can replay the list verbatim.
 */
export const TextBlock = Schema.Struct({
  type: Schema.Literal("text"),
  text: Schema.String,
});

/** `data` is base64; `uri` covers linked (not embedded) images. */
export const ImageBlock = Schema.Struct({
  type: Schema.Literal("image"),
  data: Schema.optional(Schema.String),
  mimeType: Schema.optional(Schema.String),
  uri: Schema.optional(Schema.String),
});

export const AudioBlock = Schema.Struct({
  type: Schema.Literal("audio"),
  data: Schema.optional(Schema.String),
  mimeType: Schema.optional(Schema.String),
});

/**
 * A file the message references (`uri`/`name` — ACP `resource_link`) or
 * embeds (`text`/`data` — ACP `resource`, base64 for `data`).
 */
export const FileBlock = Schema.Struct({
  type: Schema.Literal("file"),
  uri: Schema.optional(Schema.String),
  name: Schema.optional(Schema.String),
  mimeType: Schema.optional(Schema.String),
  size: Schema.optional(Schema.Number),
  text: Schema.optional(Schema.String),
  data: Schema.optional(Schema.String),
});

export const Block = Schema.Union(TextBlock, ImageBlock, AudioBlock, FileBlock);
export type Block = Schema.Schema.Type<typeof Block>;

/**
 * Display text recorded in `thinking` when a store held reasoning it cannot
 * show — Cursor `redacted-reasoning`, Claude `redacted_thinking`. The opaque
 * blob rides in `thinkingSignature`.
 */
export const REDACTED_THINKING = "[redacted]";

export class PromptHistoryEntry extends Schema.Class<PromptHistoryEntry>("PromptHistoryEntry")({
  content: Schema.String,
  timestamp: Schema.Number,
  isShell: Schema.Boolean.pipe(Schema.optionalWith({ default: () => false })),
}) {}

export class MessageNode extends Schema.Class<MessageNode>("MessageNode")({
  nodeId: Schema.Number,
  parentNodeId: Schema.OptionFromSelf(Schema.Number).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  role: Role,
  content: Schema.String,
  /**
   * The message's content blocks, populated only when the store recorded
   * non-text content (images, file attachments). `content` remains the
   * canonical text projection — joined text blocks — so readers that don't
   * know about blocks lose nothing.
   */
  blocks: Schema.Array(Block).pipe(Schema.optionalWith({ default: () => [] })),
  toolCalls: Schema.Array(ToolCall).pipe(Schema.optionalWith({ default: () => [] })),
  toolCallId: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  toolName: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  thinking: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /**
   * The opaque provider seal for `thinking` — Devin `thinking.signature`
   * (`sealed.v1.…`), Claude's `signature` on `thinking` blocks or `data` on
   * `redacted_thinking`, Cursor's `redacted-reasoning.data`. Never decoded,
   * preserved verbatim so a converted session replays signed thinking;
   * writers echo it back or drop the block entirely (unsigned thinking is
   * rejected on replay). When several sealed blocks fold into one node the
   * last signature wins.
   */
  thinkingSignature: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /** Token metrics the store recorded for this message (assistant turns mostly). */
  usage: Schema.OptionFromSelf(TokenUsage).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /** Model that generated this message; the session-level `model` is the default. */
  model: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  requestId: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  finishReason: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /** On `role: "tool"` nodes: how the call this result answers ended. */
  toolResult: Schema.OptionFromSelf(ToolResultInfo).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  createdAt: Schema.Number,
  metadata: Schema.Unknown,
}) {}

export class Session extends Schema.Class<Session>("Session")({
  id: Schema.String,
  title: Schema.String,
  workingDirectory: Schema.String,
  backendType: Schema.String.pipe(Schema.optionalWith({ default: () => "windsurf" })),
  agentMode: Schema.String.pipe(Schema.optionalWith({ default: () => "accept-edits" })),
  model: Schema.String,
  createdAt: Schema.Number,
  lastActivityAt: Schema.Number,
  mainChainId: Schema.Number,
  shellLastSeenIndex: Schema.Number.pipe(Schema.optionalWith({ default: () => 0 })),
  cogsJson: Schema.String.pipe(Schema.optionalWith({ default: () => "[]" })),
  workspaceDirs: Schema.String.pipe(Schema.optionalWith({ default: () => "[]" })),
  hidden: Schema.Number.pipe(Schema.optionalWith({ default: () => 0 })),
  /**
   * The session that spawned this one, when the store records a sub-agent
   * tree (Cline `parent_session_id`, Devin `subagent_heads`).
   */
  parentSessionId: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /**
   * Sub-agent identity within the parent session's team (Cline `agent_id`);
   * unrelated to the agent runtime that resumes the session.
   */
  agentId: Schema.OptionFromSelf(Schema.String).pipe(
    Schema.optionalWith({ default: () => Option.none() }),
  ),
  /**
   * Workspace snapshot refs the store recorded for this session (Cline's
   * shadow-git `metadata.checkpoint` history). References only — the
   * snapshots live in the agent's own store, resolvable via `ref`.
   */
  checkpoints: Schema.Array(CheckpointRef).pipe(Schema.optionalWith({ default: () => [] })),
  metadata: Schema.Unknown,
  nodes: Schema.Array(MessageNode).pipe(Schema.optionalWith({ default: () => [] })),
  promptHistory: Schema.Array(PromptHistoryEntry).pipe(Schema.optionalWith({ default: () => [] })),
}) {}

export class StorageError extends Schema.TaggedError<StorageError>()("StorageError", {
  message: Schema.String,
}) {}

export class ConversionError extends Schema.TaggedError<ConversionError>()("ConversionError", {
  message: Schema.String,
  cause: Schema.Unknown,
}) {}
