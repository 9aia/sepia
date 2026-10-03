import { Effect, Either } from "effect";
import { EventType, encodeSse, sseHeaders, type Event } from "sepia-agui";
import type { ControlPlaneService, HistoryMessage } from "sepia-session-control";
import { DEFAULT_KEEPALIVE_MS, SseChannel } from "./sse-channel";

export interface AguiAgentOptions {
  readonly keepAliveMs?: number;
  /** Runs effects; pass `runtime.runPromise` so spans/metrics reach the OTLP runtime. */
  readonly run?: <A, E>(effect: Effect.Effect<A, E>) => Promise<A>;
}

const SNAPSHOT_HISTORY_LIMIT = 100;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const messageOf = (error: unknown): string =>
  error instanceof Error
    ? error.message
    : isRecord(error) && "message" in error
      ? String(error.message)
      : String(error);

const lastUserText = (messages: unknown): string | null => {
  if (!Array.isArray(messages)) return null;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      isRecord(message) &&
      message.role === "user" &&
      typeof message.content === "string" &&
      message.content.trim() !== ""
    ) {
      return message.content;
    }
  }
  return null;
};

/** The session id travels on the query, in forwarded props, or as the thread id. */
const readSessionId = (request: Request, input: Record<string, unknown>): string | null => {
  const fromQuery = new URL(request.url).searchParams.get("sessionId");
  if (fromQuery !== null && fromQuery !== "") return fromQuery;
  for (const key of ["forwardedProps", "properties"] as const) {
    const carrier = input[key];
    if (isRecord(carrier) && typeof carrier.sessionId === "string" && carrier.sessionId !== "") {
      return carrier.sessionId;
    }
  }
  return typeof input.threadId === "string" && input.threadId !== "" ? input.threadId : null;
};

const snapshotMessage = (
  message: HistoryMessage,
  index: number,
): {
  readonly id: string;
  readonly role: "user" | "system" | "assistant";
  readonly content: string;
} => ({
  id: `sepia-history-${index}`,
  role: message.role === "user" || message.role === "system" ? message.role : "assistant",
  content: message.content,
});

const json = (body: unknown, status: number): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

/**
 * A standards-shaped AG-UI agent: accepts a `RunAgentInput` POST and streams
 * AG-UI events. The last user message is forwarded to the control plane as a
 * prompt; control-plane events are re-tagged with the caller's thread/run ids.
 */
export const createAguiAgentHandler =
  (plane: ControlPlaneService, options: AguiAgentOptions = {}) =>
  async (request: Request): Promise<Response> => {
    const run = options.run ?? Effect.runPromise;
    let input: unknown;
    try {
      input = await request.json();
    } catch {
      return json({ error: "Invalid JSON body" }, 400);
    }
    if (!isRecord(input)) return json({ error: "Expected a JSON object body" }, 400);

    const sessionId = readSessionId(request, input);
    if (sessionId === null) {
      return json({ error: "sessionId is required (forwardedProps.sessionId)" }, 400);
    }
    const text = lastUserText(input.messages);
    if (text === null) return json({ error: "A user message is required" }, 400);

    const threadId = typeof input.threadId === "string" ? input.threadId : sessionId;
    const runId = typeof input.runId === "string" ? input.runId : crypto.randomUUID();

    let attached;
    try {
      attached = await run(plane.attach(sessionId));
    } catch (error) {
      return json({ error: messageOf(error) }, 400);
    }
    if (!attached.attached) {
      return json(
        { error: `Session ${sessionId} is locked by another process; attach with takeover` },
        409,
      );
    }

    let unsubscribe = () => {};
    let finished = false;
    const channel = new SseChannel({
      keepAliveMs: options.keepAliveMs ?? DEFAULT_KEEPALIVE_MS,
      onTerminate: () => unsubscribe(),
    });

    const finish = (): void => {
      if (finished) return;
      finished = true;
      channel.close();
    };

    // A disconnected client should not leave the agent working on an abandoned turn.
    const cancelTurn = (): void => {
      void run(plane.cancel(sessionId)).catch(() => undefined);
    };

    const emit = (events: ReadonlyArray<Event>): void => {
      if (channel.isTerminated) return;
      const tagged = events.map((event) => ({ ...event, threadId, runId }) as Event);
      channel.push(encodeSse(tagged));
    };

    // Preload the stored backlog so the chat UI starts with context. The
    // snapshot must not precede RUN_STARTED — clients allocate the run's
    // message array on RUN_STARTED, so an early MESSAGES_SNAPSHOT crashes
    // them. Buffer it and flush right after the run opens.
    let snapshot: Event[] | null = null;

    const listener = (events: ReadonlyArray<Event>): void => {
      emit(events);
      if (snapshot !== null && events.some((event) => event.type === EventType.RUN_STARTED)) {
        emit(snapshot);
        snapshot = null;
      }
      if (events.some((event) => event.type === EventType.RUN_FINISHED)) finish();
    };

    try {
      unsubscribe = await run(plane.subscribe(sessionId, listener));
    } catch (error) {
      channel.close();
      return json({ error: messageOf(error) }, 400);
    }

    const backlog = await run(
      Effect.either(plane.getHistory(sessionId, { limit: SNAPSHOT_HISTORY_LIMIT })),
    );
    if (Either.isRight(backlog) && backlog.right.messages.length > 0) {
      snapshot = [
        {
          type: EventType.MESSAGES_SNAPSHOT,
          threadId,
          runId,
          messages: backlog.right.messages.map(snapshotMessage),
        } as Event,
      ];
    }

    const promptRun = run(Effect.either(plane.prompt(sessionId, text)));

    // A busy rejection is synchronous; race one tick so we can answer 409 before
    // committing to a stream instead of surfacing it as a RUN_ERROR event.
    const early = await Promise.race([
      promptRun,
      new Promise<null>((resolve) => setTimeout(() => resolve(null), 0)),
    ]);
    if (early !== null && Either.isLeft(early) && early.left.code === "busy") {
      channel.close();
      return json({ error: messageOf(early.left), code: "busy" }, 409);
    }

    const stream = new ReadableStream<Uint8Array>(
      {
        start(streamController) {
          channel.start(streamController);
          const abort = () => {
            cancelTurn();
            finish();
          };
          if (request.signal.aborted) abort();
          else request.signal.addEventListener("abort", abort, { once: true });
        },
        pull() {
          channel.onPull();
        },
        cancel() {
          cancelTurn();
          finish();
        },
      },
      { highWaterMark: 1 },
    );

    void promptRun.then((result) => {
      if (Either.isLeft(result)) {
        emit([
          {
            type: EventType.RUN_ERROR,
            threadId,
            runId,
            message: messageOf(result.left),
          } as Event,
        ]);
        finish();
      }
    });

    return new Response(stream, { headers: sseHeaders });
  };
