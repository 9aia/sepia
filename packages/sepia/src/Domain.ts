import { Option, Schema } from "effect";

export const Role = Schema.Literal("system", "user", "assistant", "tool");

export class ToolCall extends Schema.Class<ToolCall>("ToolCall")({
  id: Schema.String,
  name: Schema.String,
  arguments: Schema.Unknown,
  index: Schema.Number.pipe(Schema.optionalWith({ default: () => 0 })),
  kind: Schema.String.pipe(Schema.optionalWith({ default: () => "function" })),
}) {}

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
