import { randomUUID } from "node:crypto";
import { EventType } from "@ag-ui/core";
import type { Event } from "@ag-ui/core";
import type { AcpSessionUpdate, PermissionRequest } from "sepia-acp";

export interface Translator {
  startRun(): Event[];
  translate(update: AcpSessionUpdate): Event[];
  endTurn(): Event[];
  permissionRequest(request: PermissionRequest): Event[];
}

export function createTranslator(options?: {
  threadId?: string;
  runId?: string;
  messageId?: string;
}): Translator {
  const threadId = options?.threadId ?? `thread_${randomUUID()}`;
  const runId = options?.runId ?? `run_${randomUUID()}`;
  const messageIdBase = options?.messageId ?? `message_${randomUUID()}`;

  let openMessageId: string | null = null;
  let openReasoningId: string | null = null;
  const openToolCalls = new Set<string>();
  let messageSeq = 0;
  let turnEnded = false;

  const nextMessageId = (): string => {
    messageSeq += 1;
    return `${messageIdBase}_${messageSeq}`;
  };

  const toJson = (value: unknown): string => JSON.stringify(value) ?? "null";

  const custom = (name: string, value: unknown): Event => ({
    type: EventType.CUSTOM,
    name,
    value,
  });

  const closeText = (events: Event[]): void => {
    if (openMessageId === null) return;
    events.push({ type: EventType.TEXT_MESSAGE_END, messageId: openMessageId });
    openMessageId = null;
  };

  const closeReasoning = (events: Event[]): void => {
    if (openReasoningId === null) return;
    events.push({ type: EventType.REASONING_MESSAGE_END, messageId: openReasoningId });
    openReasoningId = null;
  };

  const closeToolCalls = (events: Event[]): void => {
    for (const toolCallId of openToolCalls) {
      events.push({ type: EventType.TOOL_CALL_END, toolCallId });
    }
    openToolCalls.clear();
  };

  const translate = (update: AcpSessionUpdate): Event[] => {
    switch (update.kind) {
      case "agent_message_chunk": {
        const events: Event[] = [];
        closeReasoning(events);
        if (openMessageId === null) {
          openMessageId = nextMessageId();
          events.push({
            type: EventType.TEXT_MESSAGE_START,
            messageId: openMessageId,
            role: "assistant",
          });
        }
        events.push({
          type: EventType.TEXT_MESSAGE_CONTENT,
          messageId: openMessageId,
          delta: update.text,
        });
        return events;
      }
      case "agent_thought_chunk": {
        const events: Event[] = [];
        closeText(events);
        if (openReasoningId === null) {
          openReasoningId = nextMessageId();
          events.push({
            type: EventType.REASONING_MESSAGE_START,
            messageId: openReasoningId,
            role: "reasoning",
          });
        }
        events.push({
          type: EventType.REASONING_MESSAGE_CONTENT,
          messageId: openReasoningId,
          delta: update.text,
        });
        return events;
      }
      case "user_message_chunk":
        // The client already renders the user's own input, so echoing it here would duplicate it.
        return [];
      case "tool_call": {
        const events: Event[] = [];
        closeText(events);
        closeReasoning(events);
        // `locations`/`diffs`/`contents` are non-standard fields — the AG-UI
        // schemas are passthrough, so they ride the event to the client verbatim.
        events.push({
          type: EventType.TOOL_CALL_START,
          toolCallId: update.toolCallId,
          toolCallName: update.title,
          ...(update.locations.length === 0 ? {} : { locations: update.locations }),
          ...(update.diffs.length === 0 ? {} : { diffs: update.diffs }),
          ...(update.contents === undefined || update.contents.length === 0
            ? {}
            : { contents: update.contents }),
        } as Event);
        events.push({
          type: EventType.TOOL_CALL_ARGS,
          toolCallId: update.toolCallId,
          delta: toJson(update.rawInput),
        });
        openToolCalls.add(update.toolCallId);
        return events;
      }
      case "tool_call_update": {
        const events: Event[] = [];
        if (!openToolCalls.has(update.toolCallId)) {
          // Attached mid-call (or an agent that skips `tool_call`) — the
          // START fired before this translator existed. Synthesize it so
          // the stream stays well-formed; `title` names the call when the
          // update carries one, else the id stands in as a placeholder
          // until a real START (or a later titled update) renames it.
          events.push({
            type: EventType.TOOL_CALL_START,
            toolCallId: update.toolCallId,
            toolCallName: update.title ?? update.toolCallId,
          } as Event);
          openToolCalls.add(update.toolCallId);
        }
        // A title riding an update is forwarded on each emitted frame so a
        // placeholder-named row downstream can pick the real name up late.
        const name = update.title === undefined ? {} : { toolCallName: update.title };
        if (update.rawInput !== undefined) {
          events.push({
            type: EventType.TOOL_CALL_ARGS,
            toolCallId: update.toolCallId,
            delta: toJson(update.rawInput),
            ...name,
          } as Event);
        }
        const { locations, diffs, contents } = update;
        if (update.status === "completed" || update.status === "failed") {
          if (update.rawOutput !== undefined) {
            events.push({
              type: EventType.TOOL_CALL_RESULT,
              messageId: nextMessageId(),
              toolCallId: update.toolCallId,
              content: toJson(update.rawOutput),
            });
          }
          // `status` (and the title/name + file payloads) aren't declared
          // ToolCallEndEvent fields — build as a record so the passthrough
          // extras still ride to the client (applyAguiEvent reads
          // `event.toolStatus ?? event.status`).
          const end: Record<string, unknown> = {
            type: EventType.TOOL_CALL_END,
            toolCallId: update.toolCallId,
            status: update.status,
            ...name,
            ...(locations === undefined ? {} : { locations }),
            ...(diffs === undefined ? {} : { diffs }),
            ...(contents === undefined ? {} : { contents }),
          };
          events.push(end as Event);
          openToolCalls.delete(update.toolCallId);
        } else if (locations !== undefined || diffs !== undefined || contents !== undefined) {
          // A mid-call file/content update has no AG-UI tool event to ride — a
          // named custom event carries it so the live row can fold it in.
          events.push(
            custom("acp:tool_call_update", {
              toolCallId: update.toolCallId,
              ...(update.title === undefined ? {} : { title: update.title }),
              ...(locations === undefined ? {} : { locations }),
              ...(diffs === undefined ? {} : { diffs }),
              ...(contents === undefined ? {} : { contents }),
            }),
          );
        }
        return events;
      }
      case "plan":
        return [custom("acp:plan", update)];
      case "current_mode_update":
        return [custom("acp:current_mode_update", update)];
      case "available_commands_update":
        return [custom("acp:available_commands_update", update)];
      case "other":
        return [custom(`acp:${update.sessionUpdate}`, update.raw)];
    }
  };

  return {
    startRun(): Event[] {
      openMessageId = null;
      openReasoningId = null;
      openToolCalls.clear();
      turnEnded = false;
      return [{ type: EventType.RUN_STARTED, threadId, runId }];
    },
    translate,
    endTurn(): Event[] {
      if (turnEnded) return [];
      turnEnded = true;
      const events: Event[] = [];
      closeText(events);
      closeReasoning(events);
      closeToolCalls(events);
      events.push({ type: EventType.RUN_FINISHED, threadId, runId });
      return events;
    },
    permissionRequest(request: PermissionRequest): Event[] {
      return [custom("acp:permission_request", request)];
    },
  };
}
