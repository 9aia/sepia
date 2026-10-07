import { sseHeaders } from "sepia-agui";
import type { NodeEventFeed } from "../events";
import { SseChannel } from "../sse-channel";
import { segmentsEqual, type RouteHandler } from "./shared";

const frame = (kind: string, payload: Record<string, unknown>): string =>
  `event: ${kind}\ndata: ${JSON.stringify(payload)}\n\n`;

/**
 * `GET /api/events` — the node-level feed (docs/protocol.md). Each
 * connection drains a bounded subscription into an SseChannel; the channel
 * kills the stream when the client stops draining, so a slow consumer never
 * wedges the feed (clients refetch on reconnect anyway). Heartbeats ride
 * the stream as real `heartbeat` events rather than comment pings.
 */
const eventsResponse = (
  feed: NodeEventFeed,
  signal: AbortSignal,
  cors: Record<string, string>,
  keepAliveMs: number,
): Response => {
  const sub = feed.subscribe();
  let heartbeat: ReturnType<typeof setInterval> | null = null;
  const stop = (): void => {
    sub.close();
    if (heartbeat !== null) {
      clearInterval(heartbeat);
      heartbeat = null;
    }
  };
  const channel = new SseChannel({ keepAliveMs: 0, onTerminate: stop });

  void (async () => {
    for (;;) {
      const event = await sub.next();
      if (event === undefined) return;
      channel.push(frame(event.kind, event.payload));
    }
  })();

  if (keepAliveMs > 0) {
    heartbeat = setInterval(() => {
      channel.push(frame("heartbeat", { ts: Date.now() }));
    }, keepAliveMs);
    heartbeat.unref?.();
  }

  const stream = new ReadableStream<Uint8Array>(
    {
      start(controller) {
        channel.start(controller);
        const close = () => {
          stop();
          channel.close();
          try {
            controller.close();
          } catch {
            // Already closed by the consumer.
          }
        };
        if (signal.aborted) close();
        else signal.addEventListener("abort", close, { once: true });
      },
      pull() {
        channel.onPull();
      },
      cancel() {
        stop();
        channel.close();
      },
    },
    { highWaterMark: 1 },
  );

  return new Response(stream, { headers: { ...sseHeaders, ...cors } });
};

export interface EventsRouteDeps {
  readonly feed: NodeEventFeed;
  readonly keepAliveMs: number;
}

/**
 * The node event feed — same CORS + bearer rules as /api/sessions/:id/stream
 * (EventSource clients authenticate via ?access_token).
 */
export const createEventsRoute =
  (deps: EventsRouteDeps): RouteHandler =>
  ({ request, method, segments, cors }) => {
    if (method === "GET" && segmentsEqual(segments, ["api", "events"])) {
      return eventsResponse(deps.feed, request.signal, cors, deps.keepAliveMs);
    }
    return undefined;
  };
